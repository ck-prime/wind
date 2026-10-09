//! Primary-display screenshot.
//!
//! One capture copies a frame from the persistent Windows Graphics Capture
//! session and returns PNG bytes. Health checks only ask whether a monitor
//! can be enumerated.

use std::time::SystemTime;

use wind_core::CapabilityStatus;
use wind_screenshot::{ImageFormat, Screenshot, ScreenshotError};
use xcap::Monitor;

use crate::classify::{classify, FailureKind};

pub fn screen_capture_status() -> CapabilityStatus {
    match primary_monitor() {
        Ok(_) => CapabilityStatus::Available,
        Err(error) => status_from_reason(&error),
    }
}

pub fn capture_full_display() -> Result<Screenshot, ScreenshotError> {
    let (pixels, width, height) =
        crate::wgc::capture_primary_rgba().map_err(|reason| classified(&reason))?;
    if width == 0 || height == 0 {
        return Err(ScreenshotError::NoDisplayAvailable {
            reason: "the primary display has no pixel size".to_string(),
        });
    }
    let data = encode_png(&pixels, width, height)?;
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

#[cfg(test)]
pub(crate) fn display_count() -> Result<usize, String> {
    Monitor::all()
        .map(|monitors| monitors.len())
        .map_err(|error| error.to_string())
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

fn classified(reason: &str) -> ScreenshotError {
    let owned = reason.to_string();
    match classify(reason) {
        FailureKind::Permission => ScreenshotError::PermissionRequired {
            state: wind_core::PermissionState::Denied,
            reason: owned,
        },
        FailureKind::Unsupported => ScreenshotError::Unsupported { reason: owned },
        FailureKind::NoDisplay => ScreenshotError::NoDisplayAvailable { reason: owned },
        FailureKind::Capture => ScreenshotError::CaptureFailed { reason: owned },
    }
}

fn status_from_reason(reason: &str) -> CapabilityStatus {
    match classify(reason) {
        FailureKind::Permission => CapabilityStatus::PermissionRequired,
        FailureKind::Unsupported => CapabilityStatus::Unsupported,
        FailureKind::NoDisplay | FailureKind::Capture => CapabilityStatus::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::capture_full_display;
    use std::time::Instant;
    const PNG_SIGNATURE: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

    fn png_ihdr(data: &[u8]) -> (u32, u32) {
        let width = u32::from_be_bytes(data[16..20].try_into().unwrap_or([0, 0, 0, 0]));
        let height = u32::from_be_bytes(data[20..24].try_into().unwrap_or([0, 0, 0, 0]));
        (width, height)
    }

    #[test]
    fn primary_display_capture_is_a_png() {
        let started = Instant::now();
        let shot = capture_full_display().expect("primary display capture");
        let elapsed = started.elapsed();
        assert!(shot.data.starts_with(PNG_SIGNATURE));
        assert!(shot.width > 0 && shot.height > 0);
        let (png_width, png_height) = png_ihdr(&shot.data);
        assert_eq!((png_width, png_height), (shot.width, shot.height));
        let again = capture_full_display().expect("second primary display capture");
        assert_eq!((again.width, again.height), (shot.width, shot.height));
        assert!(again.data.starts_with(PNG_SIGNATURE));
        eprintln!(
            "windows screenshot displays={} {}x{} bytes={} latency_ms={}",
            super::display_count().unwrap_or(0),
            shot.width,
            shot.height,
            shot.data.len(),
            elapsed.as_millis()
        );
    }
}
