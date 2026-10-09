//! Screenshot request, image, and provider contract.
//!
//! The service and tests use these types. ScreenCaptureKit, CoreGraphics, and
//! other operating-system types stay behind a provider implementation.

use std::fmt;
use std::time::SystemTime;

use wind_core::{CapabilityStatus, PermissionState};

/// What a capture asks for. Phase 5 captures one full display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenshotRequest {
    FullDisplay,
}

/// Image encoding stored with a screenshot. Phase 5 writes PNG only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
}

impl ImageFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Png => "png",
        }
    }
}

/// One captured image. The bytes are the encoded file contents.
///
/// Debug output reports the length and omits the pixels.
pub struct Screenshot {
    pub captured_at: SystemTime,
    pub width: u32,
    pub height: u32,
    pub format: ImageFormat,
    pub data: Vec<u8>,
}

impl fmt::Debug for Screenshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Screenshot")
            .field("captured_at", &self.captured_at)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("byte_len", &self.data.len())
            .finish()
    }
}

/// Platform-neutral capture failure.
///
/// The original operating-system error is kept as text. Platform types are not
/// part of this enum.
#[derive(Debug)]
pub enum ScreenshotError {
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
    AlreadyInProgress,
    Stopped,
}

impl fmt::Display for ScreenshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PermissionRequired { state, reason } => {
                write!(formatter, "screen capture permission {state}: {reason}")
            }
            Self::Unsupported { reason } => {
                write!(formatter, "screen capture unsupported: {reason}")
            }
            Self::NoDisplayAvailable { reason } => {
                write!(formatter, "no display is available: {reason}")
            }
            Self::CaptureFailed { reason } => write!(formatter, "screen capture failed: {reason}"),
            Self::EncodingFailed { reason } => {
                write!(formatter, "screenshot encoding failed: {reason}")
            }
            Self::StorageFailed { reason } => {
                write!(formatter, "screenshot storage failed: {reason}")
            }
            Self::ProviderUnavailable { reason } => {
                write!(formatter, "screenshot provider unavailable: {reason}")
            }
            Self::AlreadyInProgress => {
                formatter.write_str("a screenshot capture is already in progress")
            }
            Self::Stopped => formatter.write_str("screenshot service is not accepting captures"),
        }
    }
}

impl std::error::Error for ScreenshotError {}

/// Operating-system screenshot boundary.
///
/// Implementations capture one display when asked. They do not schedule
/// captures, upload bytes, or grant a permission.
pub trait ScreenshotProvider: Send + Sync {
    fn capture(&self, request: &ScreenshotRequest) -> Result<Screenshot, ScreenshotError>;

    fn capability_status(&self) -> CapabilityStatus;
}

/// Deterministic provider for tests. It does not capture a display.
#[derive(Debug)]
pub struct FakeScreenshotProvider {
    outcome: FakeOutcome,
}

/// Scripted result for [`FakeScreenshotProvider`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FakeOutcome {
    Success,
    PermissionRequired,
    Unsupported,
    CaptureFailed,
    ProviderUnavailable,
}

impl FakeScreenshotProvider {
    pub fn new(outcome: FakeOutcome) -> Self {
        Self { outcome }
    }
}

impl ScreenshotProvider for FakeScreenshotProvider {
    fn capture(&self, request: &ScreenshotRequest) -> Result<Screenshot, ScreenshotError> {
        match self.outcome {
            FakeOutcome::Success => {
                let _ = request;
                Ok(sample_screenshot())
            }
            FakeOutcome::PermissionRequired => Err(ScreenshotError::PermissionRequired {
                state: PermissionState::NotDetermined,
                reason: "scripted provider has not been granted screen capture".to_string(),
            }),
            FakeOutcome::Unsupported => Err(ScreenshotError::Unsupported {
                reason: "scripted provider does not implement capture".to_string(),
            }),
            FakeOutcome::CaptureFailed => Err(ScreenshotError::CaptureFailed {
                reason: "scripted capture failed".to_string(),
            }),
            FakeOutcome::ProviderUnavailable => Err(ScreenshotError::ProviderUnavailable {
                reason: "scripted provider is unavailable".to_string(),
            }),
        }
    }

    fn capability_status(&self) -> CapabilityStatus {
        match self.outcome {
            FakeOutcome::Success | FakeOutcome::CaptureFailed => CapabilityStatus::Available,
            FakeOutcome::PermissionRequired => CapabilityStatus::PermissionRequired,
            FakeOutcome::Unsupported => CapabilityStatus::Unsupported,
            FakeOutcome::ProviderUnavailable => CapabilityStatus::Unavailable,
        }
    }
}

pub(crate) fn sample_screenshot() -> Screenshot {
    Screenshot {
        captured_at: SystemTime::UNIX_EPOCH,
        width: 1,
        height: 1,
        format: ImageFormat::Png,
        data: ONE_PIXEL_PNG.to_vec(),
    }
}

/// 1×1 PNG. Tests check the signature and the stored dimensions.
pub(crate) const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
    0x00, 0x00, 0x03, 0x00, 0x01, 0x00, 0x05, 0xFE, 0x02, 0xFE, 0xDC, 0xCC, 0x59, 0xE7, 0x00, 0x00,
    0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

pub(crate) const PNG_SIGNATURE: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
