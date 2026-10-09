//! Primary-display video recording.
//!
//! The capture thread reads the X11 or portal screencast stream. Encoded
//! access units go through a two-frame queue; a full queue drops the new
//! frame. OpenH264 encodes H.264 and the `mp4` crate writes the container.
//! One latest frame is kept so a still display still advances at the
//! configured rate. This path does not require root and does not
//! inject into a compositor.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use mp4::{AvcConfig, Bytes, Mp4Config, Mp4Sample, Mp4Writer, TrackConfig};
use openh264::encoder::{Encoder, EncoderConfig};
use openh264::formats::RgbSliceU8;
use wind_core::CapabilityStatus;
use wind_video::{
    mp4_duration, VideoConfig, VideoError, VideoProvider, VideoResult, FRAME_QUEUE_CAPACITY,
};

use crate::mark_capture_proven;
use crate::screenshot::{primary_monitor, screen_capture_status};

#[derive(Clone)]
struct RawFrame {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

struct Session {
    stop: Arc<AtomicBool>,
    capture: JoinHandle<()>,
    encode: JoinHandle<()>,
    done: Receiver<Result<VideoResult, VideoError>>,
}

/// Linux recording provider. One recording at a time.
pub struct LinuxVideoProvider {
    session: Option<Session>,
}

impl LinuxVideoProvider {
    pub fn new() -> Self {
        Self { session: None }
    }
}

impl Default for LinuxVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

pub fn video_capture_status() -> CapabilityStatus {
    screen_capture_status()
}

impl VideoProvider for LinuxVideoProvider {
    fn capability_status(&self) -> CapabilityStatus {
        video_capture_status()
    }

    fn start(&mut self, output: &Path, config: &VideoConfig) -> Result<(), VideoError> {
        let config = config.validated()?;
        if self.session.is_some() {
            return Err(VideoError::AlreadyRecording);
        }
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let (frame_tx, frame_rx) = mpsc::sync_channel(FRAME_QUEUE_CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicU64::new(0));
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
        let capture = thread::Builder::new()
            .name("wind-video-capture".to_string())
            .spawn(move || {
                capture_loop(
                    config.frames_per_second,
                    frame_tx,
                    ready_tx,
                    capture_stop,
                    dropped,
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
                Err(VideoError::CaptureFailed { reason })
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

impl Drop for LinuxVideoProvider {
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
            mark_capture_proven();
            let _ = ready.send(Ok(()));
            value
        }
        Err(reason) => {
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
    _stop: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
) -> Result<VideoResult, VideoError> {
    let mut encoder: Option<Encoder> = None;
    let mut writer: Option<Mp4Writer<File>> = None;
    let mut track = 0u32;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut frame_count = 0u64;
    let sample_duration = 1000 / u64::from(frames_per_second.max(1));
    loop {
        match frames.recv_timeout(Duration::from_millis(50)) {
            Ok(Some(frame)) => {
                if encoder.is_none() {
                    width = frame.width;
                    height = frame.height;
                    let config = EncoderConfig::new(width, height);
                    encoder = Some(Encoder::with_config(config).map_err(|error| {
                        VideoError::EncodingFailed {
                            reason: error.to_string(),
                        }
                    })?);
                }
                let Some(encoder) = encoder.as_mut() else {
                    continue;
                };
                let rgb = RgbSliceU8::new(&frame.rgb, (width as usize, height as usize));
                let bitstream =
                    encoder
                        .encode(&rgb)
                        .map_err(|error| VideoError::EncodingFailed {
                            reason: error.to_string(),
                        })?;
                let annex_b = bitstream.to_vec();
                if writer.is_none() {
                    let (sps, pps) =
                        parameter_sets(&annex_b).ok_or_else(|| VideoError::EncodingFailed {
                            reason: "encoder did not produce H.264 parameter sets".to_string(),
                        })?;
                    let file = File::create(&path).map_err(|error| VideoError::StorageFailed {
                        reason: error.to_string(),
                    })?;
                    let mut mp4 = Mp4Writer::write_start(
                        file,
                        &Mp4Config {
                            major_brand: fourcc("isom")?,
                            minor_version: 512,
                            compatible_brands: vec![
                                fourcc("isom")?,
                                fourcc("iso2")?,
                                fourcc("avc1")?,
                                fourcc("mp41")?,
                            ],
                            timescale: 1000,
                        },
                    )
                    .map_err(|error| VideoError::EncodingFailed {
                        reason: error.to_string(),
                    })?;
                    track = mp4
                        .add_track(&TrackConfig::from(AvcConfig {
                            width: u16::try_from(width).unwrap_or(u16::MAX),
                            height: u16::try_from(height).unwrap_or(u16::MAX),
                            seq_param_set: sps,
                            pic_param_set: pps,
                        }))
                        .map_err(|error| VideoError::EncodingFailed {
                            reason: error.to_string(),
                        })?;
                    writer = Some(mp4);
                }
                let sample = length_prefixed(&annex_b);
                if let Some(mp4) = writer.as_mut() {
                    mp4.write_sample(
                        track,
                        &Mp4Sample {
                            start_time: frame_count * sample_duration,
                            duration: sample_duration,
                            rendering_offset: 0,
                            is_sync: frame_count == 0,
                            bytes: Bytes::copy_from_slice(&sample),
                        },
                    )
                    .map_err(|error| VideoError::EncodingFailed {
                        reason: error.to_string(),
                    })?;
                }
                frame_count = frame_count.saturating_add(1);
            }
            Ok(None) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let Some(mp4) = writer else {
        let _ = std::fs::remove_file(&path);
        return Err(VideoError::CaptureFailed {
            reason: "video capture produced no frames".to_string(),
        });
    };
    mp4.write_end()
        .map_err(|error| VideoError::EncodingFailed {
            reason: error.to_string(),
        })?;
    let byte_len = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
    let duration = std::fs::read(&path)
        .ok()
        .and_then(|bytes| mp4_duration(&bytes))
        .unwrap_or_else(|| Duration::from_millis(frame_count.saturating_mul(sample_duration)));
    if byte_len == 0 || duration.is_zero() {
        let _ = std::fs::remove_file(&path);
        return Err(VideoError::EncodingFailed {
            reason: "finalized video is empty".to_string(),
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
    if width == 0 || height == 0 {
        return None;
    }
    let mut rgb = vec![0u8; (width as usize) * (height as usize) * 3];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let source = (y * frame.width as usize + x) * 4;
            let pixel = frame.raw.get(source..source + 3)?;
            let dest = (y * width as usize + x) * 3;
            rgb[dest..dest + 3].copy_from_slice(pixel);
        }
    }
    Some(RawFrame { width, height, rgb })
}

fn parameter_sets(annex_b: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let nals = split_nals(annex_b);
    let sps = nals.iter().find(|nal| nal_type(nal) == 7)?.to_vec();
    let pps = nals.iter().find(|nal| nal_type(nal) == 8)?.to_vec();
    Some((sps, pps))
}

fn length_prefixed(annex_b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for nal in split_nals(annex_b) {
        let kind = nal_type(nal);
        if kind == 7 || kind == 8 || nal.is_empty() {
            continue;
        }
        let length = u32::try_from(nal.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&length.to_be_bytes());
        out.extend_from_slice(nal);
    }
    out
}

fn split_nals(data: &[u8]) -> Vec<&[u8]> {
    let mut found = Vec::new();
    let mut index = 0;
    let mut start = None;
    while index + 3 < data.len() {
        let long = data[index..].starts_with(&[0, 0, 0, 1]);
        let short = !long && data[index..].starts_with(&[0, 0, 1]);
        if long || short {
            if let Some(previous) = start {
                if previous < index {
                    found.push(&data[previous..index]);
                }
            }
            index += if long { 4 } else { 3 };
            start = Some(index);
        } else {
            index += 1;
        }
    }
    if let Some(previous) = start {
        if previous < data.len() {
            found.push(&data[previous..]);
        }
    }
    found
}

fn nal_type(nal: &[u8]) -> u8 {
    nal.first().copied().unwrap_or(0) & 0x1F
}

fn fourcc(text: &str) -> Result<mp4::FourCC, VideoError> {
    text.parse().map_err(|error| VideoError::EncodingFailed {
        reason: format!("mp4 brand is invalid: {error}"),
    })
}
