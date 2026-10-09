//! Primary-display screenshot.
//!
//! X11 enumeration does not grant a permission. Wayland status stays at
//! `PermissionRequired` until an explicit capture succeeds, because probing
//! the portal would open a system dialog.

use std::env;
use std::time::SystemTime;

use wind_core::CapabilityStatus;
use wind_screenshot::{ImageFormat, Screenshot, ScreenshotError};
use xcap::Monitor;

use crate::{capture_proven, mark_capture_proven};

pub fn screen_capture_status() -> CapabilityStatus {
    if capture_proven() {
        return CapabilityStatus::Available;
    }
    match session_kind() {
        Session::Headless => CapabilityStatus::Unavailable,
        Session::Wayland => CapabilityStatus::PermissionRequired,
        Session::X11 => match primary_monitor() {
            Ok(_) => CapabilityStatus::Available,
            Err(reason) => status_from_reason(&reason),
        },
    }
}

pub fn capture_full_display() -> Result<Screenshot, ScreenshotError> {
    if matches!(session_kind(), Session::Headless) {
        return Err(ScreenshotError::ProviderUnavailable {
            reason: "no X11 DISPLAY or Wayland session is available".to_string(),
        });
    }
    let monitor = primary_monitor().map_err(classified)?;
    let image = monitor
        .capture_image()
        .map_err(|error| classified(error.to_string()))?;
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 {
        return Err(ScreenshotError::NoDisplayAvailable {
            reason: "the primary display has no pixel size".to_string(),
        });
    }
    let data = encode_png(image.as_raw(), width, height)?;
    mark_capture_proven();
    Ok(Screenshot {
        captured_at: SystemTime::now(),
        width,
        height,
        format: ImageFormat::Png,
        data,
    })
}

pub(crate) fn primary_monitor() -> Result<Monitor, String> {
    let monitors = Monitor::all().map_err(|error| error.to_string())?;
    if let Some(primary) = monitors
        .iter()
        .find(|monitor| monitor.is_primary().unwrap_or(false))
    {
        return Ok(primary.clone());
    }
    monitors
        .into_iter()
        .next()
        .ok_or_else(|| "no primary display is available".to_string())
}

enum Session {
    Headless,
    Wayland,
    X11,
}

fn session_kind() -> Session {
    if env::var_os("WAYLAND_DISPLAY").is_some() {
        Session::Wayland
    } else if env::var_os("DISPLAY").is_some() {
        Session::X11
    } else {
        Session::Headless
    }
}

fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, ScreenshotError> {
    let expected = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4);
    if rgba.len() < expected {
        return Err(ScreenshotError::EncodingFailed {
            reason: "captured image buffer is shorter than its dimensions".to_string(),
        });
    }
    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut encoded, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer =
            encoder
                .write_header()
                .map_err(|error| ScreenshotError::EncodingFailed {
                    reason: error.to_string(),
                })?;
        writer
            .write_image_data(&rgba[..expected])
            .map_err(|error| ScreenshotError::EncodingFailed {
                reason: error.to_string(),
            })?;
    }
    Ok(encoded)
}

fn classified(reason: String) -> ScreenshotError {
    let lower = reason.to_ascii_lowercase();
    if lower.contains("permission") || lower.contains("denied") || lower.contains("portal") {
        ScreenshotError::PermissionRequired {
            state: wind_core::PermissionState::NotDetermined,
            reason,
        }
    } else if lower.contains("unsupported") || lower.contains("not supported") {
        ScreenshotError::Unsupported { reason }
    } else if lower.contains("no primary") || lower.contains("no display") {
        ScreenshotError::NoDisplayAvailable { reason }
    } else {
        ScreenshotError::CaptureFailed { reason }
    }
}

fn status_from_reason(reason: &str) -> CapabilityStatus {
    match classified(reason.to_string()) {
        ScreenshotError::PermissionRequired { .. } => CapabilityStatus::PermissionRequired,
        ScreenshotError::Unsupported { .. } => CapabilityStatus::Unsupported,
        _ => CapabilityStatus::Unavailable,
    }
}
