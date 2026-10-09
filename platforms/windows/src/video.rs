//! Primary-display video recording.
//!
//! Windows Graphics Capture feeds a bounded queue. A still display repeats
//! the latest frame at the configured rate. Media Foundation encodes
//! H.264 into an MP4 file on a separate thread. A full queue drops the new
//! frame. The supervisor thread does not encode.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use wind_core::CapabilityStatus;
use wind_video::{
    mp4_duration, VideoConfig, VideoError, VideoProvider, VideoResult, FRAME_QUEUE_CAPACITY,
};

use crate::encode::H264Sink;
use crate::screenshot::{primary_monitor, screen_capture_status};

#[derive(Clone)]
struct RawFrame {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

struct Session {
    stop: Arc<AtomicBool>,
    capture: JoinHandle<()>,
    encode: JoinHandle<()>,
    done: Receiver<Result<VideoResult, VideoError>>,
}

/// Windows recording provider. One recording at a time.
pub struct WindowsVideoProvider {
    session: Option<Session>,
}

impl WindowsVideoProvider {
    pub fn new() -> Self {
        Self { session: None }
    }
}

impl Default for WindowsVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

pub fn video_capture_status() -> CapabilityStatus {
    screen_capture_status()
}

impl VideoProvider for WindowsVideoProvider {
    fn capability_status(&self) -> CapabilityStatus {
        video_capture_status()
    }

    fn start(&mut self, output: &Path, config: &VideoConfig) -> Result<(), VideoError> {
        let config = config.validated()?;
        if self.session.is_some() {
            return Err(VideoError::AlreadyRecording);
        }
        if self.capability_status() != CapabilityStatus::Available {
            return Err(VideoError::ProviderUnavailable {
                reason: "windows display capture is not available".to_string(),
            });
        }
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let (frame_tx, frame_rx) = mpsc::sync_channel(FRAME_QUEUE_CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicU64::new(0));
        let failure: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let output_path = output.to_path_buf();
        let encode_stop = Arc::clone(&stop);
        let encode_dropped = Arc::clone(&dropped);
        let encode = thread::Builder::new()
            .name("wind-video-encode".to_string())
            .spawn(move || {
                let result = encode_loop(
                    output_path,
                    config.frames_per_second,
                    frame_rx,
                    encode_stop,
                    encode_dropped,
                );
                let _ = done_tx.send(result);
            })
            .map_err(|error| VideoError::CaptureFailed {
                reason: format!("video encoder thread could not start: {error}"),
            })?;
        let capture_stop = Arc::clone(&stop);
        let capture_failure = Arc::clone(&failure);
        let capture = thread::Builder::new()
            .name("wind-video-capture".to_string())
            .spawn(move || {
                capture_loop(
                    config.frames_per_second,
                    frame_tx,
                    ready_tx,
                    capture_stop,
                    dropped,
                    capture_failure,
                );
            })
            .map_err(|error| VideoError::CaptureFailed {
                reason: format!("video capture thread could not start: {error}"),
            })?;
        match ready_rx.recv_timeout(Duration::from_secs(8)) {
            Ok(Ok(())) => {
                self.session = Some(Session {
                    stop,
                    capture,
                    encode,
                    done: done_rx,
                });
                Ok(())
            }
            Ok(Err(reason)) => {
                stop.store(true, Ordering::Release);
                let _ = capture.join();
                let _ = encode.join();
                Err(screenshot_error_as_video(&reason))
            }
            Err(_) => {
                stop.store(true, Ordering::Release);
                let _ = capture.join();
                let _ = encode.join();
                Err(VideoError::CaptureFailed {
                    reason: "video capture did not start".to_string(),
                })
            }
        }
    }

    fn stop(&mut self) -> Result<VideoResult, VideoError> {
        let Some(session) = self.session.take() else {
            return Err(VideoError::NotRecording);
        };
        session.stop.store(true, Ordering::Release);
        let _ = session.capture.join();
        let _ = session.encode.join();
        session.done.recv().unwrap_or_else(|_| {
            Err(VideoError::EncodingFailed {
                reason: "video encoder ended without a result".to_string(),
            })
        })
    }

    fn is_recording(&self) -> bool {
        self.session.is_some()
    }
}

impl Drop for WindowsVideoProvider {
    fn drop(&mut self) {
        if self.is_recording() {
            let _ = self.stop();
        }
    }
}

fn capture_loop(
    frames_per_second: u32,
    frames: SyncSender<Option<RawFrame>>,
    ready: mpsc::Sender<Result<(), String>>,
    stop: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
    failure: Arc<Mutex<Option<String>>>,
) {
    let interval = Duration::from_millis(1000 / u64::from(frames_per_second.max(1)));
    let started: Result<(xcap::VideoRecorder, std::sync::mpsc::Receiver<xcap::Frame>), String> =
        (|| {
            let monitor = primary_monitor()?;
            let (recorder, receiver) = monitor
                .video_recorder()
                .map_err(|error| error.to_string())?;
            recorder.start().map_err(|error| error.to_string())?;
            Ok((recorder, receiver))
        })();
    let (recorder, receiver) = match started {
        Ok(value) => {
            let _ = ready.send(Ok(()));
            value
        }
        Err(reason) => {
            *failure
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(reason.clone());
            let _ = ready.send(Err(reason));
            let _ = frames.send(None);
            return;
        }
    };
    let mut next = Instant::now();
    let mut latest = None;
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        match receiver.recv_timeout(Duration::from_millis(20)) {
            Ok(frame) => {
                if let Some(raw) = unpack_frame(frame) {
                    latest = Some(raw);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        let now = Instant::now();
        if now < next {
            continue;
        }
        let Some(raw) = latest.clone() else {
            continue;
        };
        next = now + interval;
        match frames.try_send(Some(raw)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                dropped.fetch_add(1, Ordering::Release);
            }
            Err(TrySendError::Disconnected(_)) => break,
        }
    }
    let _ = recorder.stop();
    let _ = frames.send(None);
}

fn encode_loop(
    path: PathBuf,
    frames_per_second: u32,
    frames: Receiver<Option<RawFrame>>,
    stop: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
) -> Result<VideoResult, VideoError> {
    let mut sink: Option<H264Sink> = None;
    let mut width = 0u32;
    let mut height = 0u32;
    loop {
        match frames.recv_timeout(Duration::from_millis(50)) {
            Ok(Some(frame)) => {
                if sink.is_none() {
                    width = frame.width;
                    height = frame.height;
                    match H264Sink::create(&path, width, height, frames_per_second) {
                        Ok(created) => sink = Some(created),
                        Err(reason) => {
                            let _ = std::fs::remove_file(&path);
                            return Err(VideoError::EncodingFailed { reason });
                        }
                    }
                }
                if frame.width != width || frame.height != height {
                    dropped.fetch_add(1, Ordering::Release);
                    continue;
                }
                if let Some(encoder) = sink.as_mut() {
                    if let Err(reason) = encoder.write_bgra(&frame.bgra) {
                        let _ = std::fs::remove_file(&path);
                        return Err(VideoError::EncodingFailed { reason });
                    }
                }
            }
            Ok(None) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if stop.load(Ordering::Acquire) {
                    continue;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let Some(encoder) = sink else {
        let _ = std::fs::remove_file(&path);
        return Err(VideoError::CaptureFailed {
            reason: "video capture produced no frames".to_string(),
        });
    };
    let frame_count = encoder.frames();
    if let Err(reason) = encoder.finalize() {
        let _ = std::fs::remove_file(&path);
        return Err(VideoError::EncodingFailed { reason });
    }
    let byte_len = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
    if byte_len == 0 {
        let _ = std::fs::remove_file(&path);
        return Err(VideoError::EncodingFailed {
            reason: "finalized video is empty".to_string(),
        });
    }
    let duration = match read_duration(&path) {
        Ok(duration) => duration,
        Err(reason) => {
            let _ = std::fs::remove_file(&path);
            return Err(VideoError::EncodingFailed { reason });
        }
    };
    if duration.is_zero() {
        let _ = std::fs::remove_file(&path);
        return Err(VideoError::EncodingFailed {
            reason: "finalized video has no duration".to_string(),
        });
    }
    Ok(VideoResult {
        width,
        height,
        frame_count,
        dropped_frames: dropped.load(Ordering::Acquire),
        duration,
        byte_len,
    })
}

fn unpack_frame(frame: xcap::Frame) -> Option<RawFrame> {
    let width = frame.width & !1;
    let height = frame.height & !1;
    crop_bgra(&frame.raw, frame.width, width, height).map(|bgra| RawFrame {
        width,
        height,
        bgra,
    })
}

fn crop_bgra(rgba: &[u8], src_width: u32, width: u32, height: u32) -> Option<Vec<u8>> {
    if width == 0 || height == 0 {
        return None;
    }
    let mut bgra = vec![0u8; (width as usize) * (height as usize) * 4];
    for y in 0..height as usize {
        let row = y * src_width as usize * 4;
        for x in 0..width as usize {
            let source = row + x * 4;
            let dest = (y * width as usize + x) * 4;
            let pixel = rgba.get(source..source + 4)?;
            bgra[dest] = pixel[2];
            bgra[dest + 1] = pixel[1];
            bgra[dest + 2] = pixel[0];
            bgra[dest + 3] = 255;
        }
    }
    Some(bgra)
}

fn read_duration(path: &Path) -> Result<Duration, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut pending = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        pending.extend_from_slice(&chunk[..read]);
        if let Some(duration) = mp4_duration(&pending) {
            return Ok(duration);
        }
        if pending.len() > 1024 * 1024 {
            let start = pending.len() - 64;
            pending = pending.split_off(start);
        }
    }
    Err("video container has no duration".to_string())
}

pub(crate) fn screenshot_error_as_video(reason: &str) -> VideoError {
    match crate::classify::classify(reason) {
        crate::classify::FailureKind::Permission => VideoError::PermissionRequired {
            state: wind_core::PermissionState::Denied,
            reason: reason.to_string(),
        },
        crate::classify::FailureKind::Unsupported => VideoError::Unsupported {
            reason: reason.to_string(),
        },
        crate::classify::FailureKind::NoDisplay => VideoError::NoDisplayAvailable {
            reason: reason.to_string(),
        },
        crate::classify::FailureKind::Capture => VideoError::CaptureFailed {
            reason: reason.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::WindowsVideoProvider;
    use std::time::{Duration, Instant};
    use wind_video::{VideoConfig, VideoProvider};

    #[test]
    fn two_recordings_are_playable_mp4() {
        let directory =
            std::env::temp_dir().join(format!("wind-windows-video-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("dir");
        let mut provider = WindowsVideoProvider::new();
        let config = VideoConfig::default();
        for cycle in 1..=2 {
            let path = directory.join(format!("cycle-{cycle}.mp4"));
            provider.start(&path, &config).expect("start");
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(3) {
                std::thread::sleep(Duration::from_millis(100));
            }
            let result = provider.stop().expect("stop");
            let bytes = std::fs::read(&path).expect("read");
            assert!(
                bytes.windows(4).any(|window| window == b"ftyp"),
                "cycle {cycle} is not an mp4"
            );
            assert!(result.byte_len > 0, "cycle {cycle} is empty");
            assert!(result.duration > Duration::ZERO, "cycle {cycle} duration");
            assert!(result.width > 0 && result.height > 0);
            eprintln!(
                "windows video cycle={cycle} {}x{} frames={} dropped={} bytes={} duration_ms={}",
                result.width,
                result.height,
                result.frame_count,
                result.dropped_frames,
                result.byte_len,
                result.duration.as_millis()
            );
        }
        eprintln!("windows video files kept in {}", directory.display());
    }
}
