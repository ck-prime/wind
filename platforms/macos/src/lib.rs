//! macOS screenshot and video capture.
//!
//! ScreenCaptureKit takes one full-display image. `CGPreflightScreenCaptureAccess`
//! reports whether Screen Recording access is granted. It does not prompt.
//! This crate does not call `CGRequestScreenCaptureAccess` and does not read TCC.
//! System Settings opens only from an explicit settings request.

use std::time::SystemTime;

use screencapturekit::error::{SCError, SCStreamErrorCode};
use screencapturekit::screenshot_manager::{CGImageExt, SCScreenshotManager};
use screencapturekit::shareable_content::{SCShareableContent, SCWindow};
use screencapturekit::stream::configuration::SCStreamConfiguration;
use screencapturekit::stream::content_filter::SCContentFilter;
use wind_core::{
    capability_status_from_permission, Capability, CapabilityStatus, PermissionState,
    PermissionStatus,
};
mod activity;
mod app;
mod secret;
mod settings;
mod url;
mod video;

use wind_screenshot::{ImageFormat, Screenshot, ScreenshotError};

pub use activity::{input_monitoring_permission, MacosActivityProvider};
pub use app::{application_status, current_application};
pub use secret::MacosSecretStore;
pub use settings::open_system_settings;
pub use url::MacosUrlProvider;
pub use video::MacosVideoProvider;

/// Coarse Screen Recording readiness.
///
/// The public preflight API returns a boolean. `true` is granted. `false` means
/// access is not granted. The API does not tell a first prompt apart from an
/// explicit denial, so `false` is `NotDetermined` rather than a claimed denial.
pub fn screen_capture_permission() -> PermissionStatus {
    if preflight_granted() {
        PermissionStatus::new(
            Capability::ScreenCapture,
            PermissionState::Granted,
            "CGPreflightScreenCaptureAccess reports screen capture access",
        )
    } else {
        PermissionStatus::new(
            Capability::ScreenCapture,
            PermissionState::NotDetermined,
            "CGPreflightScreenCaptureAccess reports screen capture access is not granted; it does not distinguish not-determined from denied",
        )
    }
}

pub fn screen_capture_status() -> CapabilityStatus {
    capability_status_from_permission(screen_capture_permission().state)
}

/// Capture the first shareable display as PNG bytes.
///
/// The shareable-content query and the screenshot manager run only after
/// preflight reports access. Both are dropped before this function returns.
/// No stream is left running.
pub fn capture_full_display() -> Result<Screenshot, ScreenshotError> {
    if !preflight_granted() {
        let status = screen_capture_permission();
        return Err(ScreenshotError::PermissionRequired {
            state: status.state,
            reason: status
                .reason
                .unwrap_or_else(|| "screen capture access is not granted".to_string()),
        });
    }
    let content = SCShareableContent::get().map_err(map_capture_error)?;
    let displays = content.displays();
    let Some(display) = displays.first() else {
        return Err(ScreenshotError::NoDisplayAvailable {
            reason: "ScreenCaptureKit returned no displays".to_string(),
        });
    };
    let width = display.width();
    let height = display.height();
    if width == 0 || height == 0 {
        return Err(ScreenshotError::NoDisplayAvailable {
            reason: "the first display has no pixel size".to_string(),
        });
    }
    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&[] as &[SCWindow])
        .build()
        .map_err(map_capture_error)?;
    let configuration = SCStreamConfiguration::new()
        .with_width(width)
        .with_height(height);
    let image =
        SCScreenshotManager::capture_image(&filter, &configuration).map_err(map_capture_error)?;
    let (encoded_width, encoded_height, data) = encode_png(image)?;
    drop(displays);
    drop(content);
    Ok(Screenshot {
        captured_at: SystemTime::now(),
        width: encoded_width,
        height: encoded_height,
        format: ImageFormat::Png,
        data,
    })
}

fn encode_png(
    image: screencapturekit::screenshot_manager::CGImage,
) -> Result<(u32, u32, Vec<u8>), ScreenshotError> {
    let width =
        u32::try_from(image.width()).map_err(|_| encoding_error("image width is out of range"))?;
    let height = u32::try_from(image.height())
        .map_err(|_| encoding_error("image height is out of range"))?;
    if width == 0 || height == 0 {
        return Err(encoding_error("captured image has no pixels"));
    }
    let rgba = image
        .rgba_data()
        .map_err(|error| encoding_error(error.to_string()))?;
    let expected = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4);
    if rgba.len() < expected {
        return Err(encoding_error(
            "captured image buffer is shorter than its dimensions",
        ));
    }
    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut encoded, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| encoding_error(error.to_string()))?;
        writer
            .write_image_data(&rgba[..expected])
            .map_err(|error| encoding_error(error.to_string()))?;
    }
    drop(rgba);
    Ok((width, height, encoded))
}

fn encoding_error(reason: impl Into<String>) -> ScreenshotError {
    ScreenshotError::EncodingFailed {
        reason: reason.into(),
    }
}

pub(crate) fn map_capture_error(error: SCError) -> ScreenshotError {
    match error {
        SCError::PermissionDenied(reason) => ScreenshotError::PermissionRequired {
            state: PermissionState::Denied,
            reason,
        },
        SCError::NoShareableContent(reason) => ScreenshotError::PermissionRequired {
            state: PermissionState::NotDetermined,
            reason,
        },
        SCError::FeatureNotAvailable {
            feature,
            required_version,
        } => ScreenshotError::Unsupported {
            reason: format!("{feature} requires macOS {required_version} or newer"),
        },
        SCError::DisplayNotFound(reason) => ScreenshotError::NoDisplayAvailable { reason },
        SCError::SCStreamError { code, message } => map_stream_error(code, message),
        other => ScreenshotError::CaptureFailed {
            reason: other.to_string(),
        },
    }
}

fn map_stream_error(code: SCStreamErrorCode, message: Option<String>) -> ScreenshotError {
    let reason = message.unwrap_or_else(|| code.to_string());
    match code {
        SCStreamErrorCode::UserDeclined => ScreenshotError::PermissionRequired {
            state: PermissionState::Denied,
            reason,
        },
        SCStreamErrorCode::NoDisplayList | SCStreamErrorCode::NoCaptureSource => {
            ScreenshotError::NoDisplayAvailable { reason }
        }
        SCStreamErrorCode::MissingEntitlements => ScreenshotError::Unsupported { reason },
        _ => ScreenshotError::CaptureFailed { reason },
    }
}

#[allow(unsafe_code)]
pub(crate) fn preflight_granted() -> bool {
    // SAFETY: CGPreflightScreenCaptureAccess is a public CoreGraphics function.
    // It takes no pointers, returns a boolean, does not prompt, and does not
    // modify the TCC database.
    unsafe { CGPreflightScreenCaptureAccess() }
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}
