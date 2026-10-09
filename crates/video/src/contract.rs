//! Video request, result, and provider contract.
//!
//! Operating-system capture and encoder types stay behind a provider.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use wind_core::{CapabilityStatus, PermissionState};

/// Display a recording uses. Only the primary display is implemented.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DisplaySelection {
    #[default]
    Primary,
}

/// Explicit recording settings. The service supplies the output path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoConfig {
    pub frames_per_second: u32,
    pub display: DisplaySelection,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            frames_per_second: 10,
            display: DisplaySelection::Primary,
        }
    }
}

impl VideoConfig {
    pub fn validated(self) -> Result<Self, VideoError> {
        if !(1..=15).contains(&self.frames_per_second) {
            return Err(VideoError::EncodingFailed {
                reason: "frame rate must be from 1 to 15".to_string(),
            });
        }
        Ok(self)
    }
}

/// Metadata for one finalized recording. The file itself stays on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoResult {
    pub width: u32,
    pub height: u32,
    pub frame_count: u64,
    pub dropped_frames: u64,
    pub duration: Duration,
    pub byte_len: u64,
}

/// Platform-neutral recording failure.
#[derive(Debug)]
pub enum VideoError {
    PermissionRequired {
        state: PermissionState,
        reason: String,
    },
    Unsupported {
        reason: String,
    },
    NoDisplayAvailable {
        reason: String,
    },
    CaptureFailed {
        reason: String,
    },
    EncodingFailed {
        reason: String,
    },
    StorageFailed {
        reason: String,
    },
    ProviderUnavailable {
        reason: String,
    },
    AlreadyRecording,
    NotRecording,
    Stopped,
}

impl fmt::Display for VideoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PermissionRequired { reason, .. } => write!(formatter, "{reason}"),
            Self::Unsupported { reason }
            | Self::NoDisplayAvailable { reason }
            | Self::CaptureFailed { reason }
            | Self::EncodingFailed { reason }
            | Self::StorageFailed { reason }
            | Self::ProviderUnavailable { reason } => write!(formatter, "{reason}"),
            Self::AlreadyRecording => write!(formatter, "a recording is already in progress"),
            Self::NotRecording => write!(formatter, "video recording is not in progress"),
            Self::Stopped => write!(formatter, "video service is not running"),
        }
    }
}

/// Screen recording owned by one platform provider.
///
/// `start` returns when capture is running. `stop` finalizes the file.
pub trait VideoProvider: Send {
    fn capability_status(&self) -> CapabilityStatus;
    fn start(&mut self, output: &Path, config: &VideoConfig) -> Result<(), VideoError>;
    fn stop(&mut self) -> Result<VideoResult, VideoError>;
    fn is_recording(&self) -> bool;
}

/// Scripted provider for service tests. It does not touch a display.
#[derive(Debug)]
pub struct FakeVideoProvider {
    outcome: FakeOutcome,
    output: Option<PathBuf>,
    recording: bool,
}

/// What [`FakeVideoProvider`] does when a recording starts or stops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FakeOutcome {
    Success,
    PermissionRequired,
    Unsupported,
    Unavailable,
    CaptureFailed,
    EncodingFailed,
}

impl FakeVideoProvider {
    pub fn new(outcome: FakeOutcome) -> Self {
        Self {
            outcome,
            output: None,
            recording: false,
        }
    }
}

impl VideoProvider for FakeVideoProvider {
    fn capability_status(&self) -> CapabilityStatus {
        match self.outcome {
            FakeOutcome::Success | FakeOutcome::CaptureFailed | FakeOutcome::EncodingFailed => {
                CapabilityStatus::Available
            }
            FakeOutcome::PermissionRequired => CapabilityStatus::PermissionRequired,
            FakeOutcome::Unsupported => CapabilityStatus::Unsupported,
            FakeOutcome::Unavailable => CapabilityStatus::Unavailable,
        }
    }

    fn start(&mut self, output: &Path, config: &VideoConfig) -> Result<(), VideoError> {
        config.validated()?;
        if self.recording {
            return Err(VideoError::AlreadyRecording);
        }
        match self.outcome {
            FakeOutcome::PermissionRequired => {
                return Err(VideoError::PermissionRequired {
                    state: PermissionState::NotDetermined,
                    reason: "video capture permission is required".to_string(),
                });
            }
            FakeOutcome::Unsupported => {
                return Err(VideoError::Unsupported {
                    reason: "video capture is not implemented".to_string(),
                });
            }
            FakeOutcome::Unavailable => {
                return Err(VideoError::ProviderUnavailable {
                    reason: "video capture is temporarily unavailable".to_string(),
                });
            }
            FakeOutcome::CaptureFailed => {
                return Err(VideoError::CaptureFailed {
                    reason: "video capture failed".to_string(),
                });
            }
            FakeOutcome::Success | FakeOutcome::EncodingFailed => {}
        }
        self.output = Some(output.to_path_buf());
        self.recording = true;
        Ok(())
    }

    fn stop(&mut self) -> Result<VideoResult, VideoError> {
        if !self.recording {
            return Err(VideoError::NotRecording);
        }
        self.recording = false;
        let path = self
            .output
            .take()
            .ok_or_else(|| VideoError::StorageFailed {
                reason: "recording has no output path".to_string(),
            })?;
        if self.outcome == FakeOutcome::EncodingFailed {
            return Err(VideoError::EncodingFailed {
                reason: "video encoder failed".to_string(),
            });
        }
        fs::write(&path, FINALIZED_MARKER).map_err(|_| VideoError::StorageFailed {
            reason: "video file could not be written".to_string(),
        })?;
        Ok(VideoResult {
            width: 2,
            height: 2,
            frame_count: 2,
            dropped_frames: 0,
            duration: Duration::from_millis(200),
            byte_len: u64::try_from(FINALIZED_MARKER.len()).unwrap_or(0),
        })
    }

    fn is_recording(&self) -> bool {
        self.recording
    }
}

/// Bytes a successful fake recording writes. Tests use them as a finalized marker.
pub const FINALIZED_MARKER: &[u8] = b"ftypisomwind";

/// Read an MP4 `mvhd` duration. Returns `None` when the atom is absent.
pub fn mp4_duration(bytes: &[u8]) -> Option<Duration> {
    let index = bytes.windows(4).position(|window| window == b"mvhd")?;
    let version = *bytes.get(index + 4)?;
    let (timescale, duration) = if version == 0 {
        let timescale = u32::from_be_bytes(bytes.get(index + 16..index + 20)?.try_into().ok()?);
        let duration = u32::from_be_bytes(bytes.get(index + 20..index + 24)?.try_into().ok()?);
        (u64::from(timescale), u64::from(duration))
    } else {
        let timescale = u32::from_be_bytes(bytes.get(index + 28..index + 32)?.try_into().ok()?);
        let duration = u64::from_be_bytes(bytes.get(index + 32..index + 40)?.try_into().ok()?);
        (u64::from(timescale), duration)
    };
    if timescale == 0 {
        return None;
    }
    Some(Duration::from_secs_f64(duration as f64 / timescale as f64))
}

#[cfg(test)]
mod tests {
    use super::{mp4_duration, VideoConfig};
    use std::time::Duration;

    #[test]
    fn frame_rate_is_bounded() {
        assert!(VideoConfig {
            frames_per_second: 0,
            ..VideoConfig::default()
        }
        .validated()
        .is_err());
        assert!(VideoConfig::default().validated().is_ok());
    }

    #[test]
    fn mvhd_duration_is_read() {
        let mut body = vec![0u8; 24];
        body[12..16].copy_from_slice(&1000u32.to_be_bytes());
        body[16..20].copy_from_slice(&2500u32.to_be_bytes());
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&16u32.to_be_bytes());
        bytes.extend_from_slice(b"ftyp");
        bytes.extend_from_slice(b"isom");
        bytes.extend_from_slice(&((8 + body.len()) as u32).to_be_bytes());
        bytes.extend_from_slice(b"mvhd");
        bytes.extend(body);
        assert_eq!(mp4_duration(&bytes), Some(Duration::from_millis(2500)));
    }
}
