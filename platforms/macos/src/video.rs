//! Primary-display video recording.
//!
//! ScreenCaptureKit captures a frame stream. `SCRecordingOutput` encodes H.264
//! into an MP4 file with the system encoder. This is not a screenshot loop.
//! Screen Recording permission is the same preflight used by screenshots.
//! The call does not prompt and does not read the TCC database.

use std::path::{Path, PathBuf};
use std::time::Instant;

use screencapturekit::prelude::*;
use screencapturekit::recording_output::{
    SCRecordingOutput, SCRecordingOutputCodec, SCRecordingOutputConfiguration,
    SCRecordingOutputFileType,
};
use wind_core::{CapabilityStatus, PermissionState};
use wind_video::{mp4_duration, VideoConfig, VideoError, VideoProvider, VideoResult};

use crate::{map_capture_error, preflight_granted, screen_capture_status};

struct Session {
    stream: SCStream,
    output: SCRecordingOutput,
    path: PathBuf,
    width: u32,
    height: u32,
    started: Instant,
}

/// macOS recording provider. One recording at a time.
pub struct MacosVideoProvider {
    session: Option<Session>,
}

impl MacosVideoProvider {
    pub fn new() -> Self {
        Self { session: None }
    }
}

impl Default for MacosVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoProvider for MacosVideoProvider {
    fn capability_status(&self) -> CapabilityStatus {
        if !preflight_granted() {
            return CapabilityStatus::PermissionRequired;
        }
        if !SCRecordingOutput::is_available() {
            return CapabilityStatus::Unsupported;
        }
        screen_capture_status()
    }

    fn start(&mut self, output: &Path, config: &VideoConfig) -> Result<(), VideoError> {
        let _ = config.validated()?;
        if self.session.is_some() {
            return Err(VideoError::AlreadyRecording);
        }
        if !preflight_granted() {
            return Err(VideoError::PermissionRequired {
                state: PermissionState::NotDetermined,
                reason: "screen recording access is not granted".to_string(),
            });
        }
        if !SCRecordingOutput::is_available() {
            return Err(VideoError::Unsupported {
                reason: "SCRecordingOutput requires macOS 15 or newer".to_string(),
            });
        }
        let content = SCShareableContent::get().map_err(video_capture)?;
        let displays = content.displays();
        let Some(display) = displays.first() else {
            return Err(VideoError::NoDisplayAvailable {
                reason: "ScreenCaptureKit returned no displays".to_string(),
            });
        };
        let width = display.width();
        let height = display.height();
        if width == 0 || height == 0 {
            return Err(VideoError::NoDisplayAvailable {
                reason: "the first display has no pixel size".to_string(),
            });
        }
        let filter = SCContentFilter::create()
            .with_display(display)
            .build()
            .map_err(video_capture)?;
        // ScreenCaptureKit owns the recording queue and drops frames when the
        // encoder falls behind. Wind does not keep a second unbounded buffer.
        let configuration = SCStreamConfiguration::new()
            .with_width(width)
            .with_height(height);
        let recording_config = SCRecordingOutputConfiguration::new()
            .map_err(video_capture)?
            .with_output_url(output)
            .map_err(|error| VideoError::StorageFailed {
                reason: error.to_string(),
            })?
            .with_video_codec(SCRecordingOutputCodec::H264)
            .with_output_file_type(SCRecordingOutputFileType::MP4);
        let recording = SCRecordingOutput::new(&recording_config).ok_or_else(|| {
            VideoError::EncodingFailed {
                reason: "ScreenCaptureKit could not create a recording output".to_string(),
            }
        })?;
        let stream = SCStream::new(&filter, &configuration).map_err(video_capture)?;
        stream
            .add_recording_output(&recording)
            .map_err(video_capture)?;
        stream.start_capture().map_err(video_capture)?;
        self.session = Some(Session {
            stream,
            output: recording,
            path: output.to_path_buf(),
            width,
            height,
            started: Instant::now(),
        });
        Ok(())
    }

    fn stop(&mut self) -> Result<VideoResult, VideoError> {
        let Some(session) = self.session.take() else {
            return Err(VideoError::NotRecording);
        };
        let width = session.width;
        let height = session.height;
        session.stream.stop_capture().map_err(video_capture)?;
        session
            .stream
            .remove_recording_output(&session.output)
            .map_err(video_capture)?;
        let byte_len = u64::try_from(session.output.recorded_file_size()).unwrap_or(0);
        if byte_len == 0 {
            let _ = std::fs::remove_file(&session.path);
            return Err(VideoError::EncodingFailed {
                reason: "finalized video is empty".to_string(),
            });
        }
        let duration = read_duration(&session.path).unwrap_or_else(|| session.started.elapsed());
        if duration.is_zero() {
            let _ = std::fs::remove_file(&session.path);
            return Err(VideoError::EncodingFailed {
                reason: "finalized video has no duration".to_string(),
            });
        }
        Ok(VideoResult {
            width,
            height,
            frame_count: 0,
            dropped_frames: 0,
            duration,
            byte_len,
        })
    }

    fn is_recording(&self) -> bool {
        self.session.is_some()
    }
}

impl Drop for MacosVideoProvider {
    fn drop(&mut self) {
        if self.is_recording() {
            let _ = self.stop();
        }
    }
}

fn video_capture(error: screencapturekit::error::SCError) -> VideoError {
    match map_capture_error(error) {
        wind_screenshot::ScreenshotError::PermissionRequired { state, reason } => {
            VideoError::PermissionRequired { state, reason }
        }
        wind_screenshot::ScreenshotError::Unsupported { reason } => {
            VideoError::Unsupported { reason }
        }
        wind_screenshot::ScreenshotError::NoDisplayAvailable { reason } => {
            VideoError::NoDisplayAvailable { reason }
        }
        other => VideoError::CaptureFailed {
            reason: other.to_string(),
        },
    }
}

fn read_duration(path: &Path) -> Option<std::time::Duration> {
    let bytes = std::fs::read(path).ok()?;
    mp4_duration(&bytes)
}
