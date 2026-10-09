//! Foreground application identity on macOS.
//!
//! `NSWorkspace` reports the frontmost application. This does not use
//! Accessibility, Apple Events, or window titles. The bundle identifier is
//! the stable application id.

use std::time::SystemTime;

use objc2::MainThreadMarker;
use objc2_app_kit::NSWorkspace;
use wind_app_tracker::{ActiveApplication, AppError};
use wind_core::CapabilityStatus;

pub fn application_status() -> CapabilityStatus {
    CapabilityStatus::Available
}

pub fn current_application() -> Result<Option<ActiveApplication>, AppError> {
    let Some(marker) = MainThreadMarker::new() else {
        return Err(AppError::Unavailable {
            reason: "foreground application lookup must run on the main thread".to_string(),
        });
    };
    let workspace = NSWorkspace::sharedWorkspace(marker);
    let Some(application) = workspace.frontmostApplication() else {
        return Ok(None);
    };
    let bundle = application
        .bundleIdentifier()
        .map(|value| value.to_string());
    let name = application.localizedName().map(|value| value.to_string());
    let Some(application_id) = bundle.filter(|value| !value.is_empty()) else {
        return Err(AppError::Unavailable {
            reason: "frontmost application has no bundle identifier".to_string(),
        });
    };
    Ok(Some(ActiveApplication {
        application_id,
        name: name.filter(|value| !value.is_empty()),
        executable: None,
        observed_at: SystemTime::now(),
    }))
}
