//! Foreground application identity on macOS.
//!
//! `NSWorkspace` reports the frontmost application. This does not use
//! Accessibility, Apple Events, or window titles. The bundle identifier is
//! the stable application id.

use std::time::SystemTime;

use std::sync::Once;

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSWorkspace};
use wind_app_tracker::{ActiveApplication, AppError};
use wind_core::CapabilityStatus;

/// Initialize AppKit as a background application.
///
/// `NSWorkspace` needs the shared `NSApplication` on the main thread.
/// The activation policy is prohibited, so Wind does not take focus, show a
/// Dock icon, or open a window. This does not run the AppKit event loop.
pub fn prepare_background_application() {
    let Some(marker) = MainThreadMarker::new() else {
        return;
    };
    static PREPARED: Once = Once::new();
    PREPARED.call_once(|| {
        let application = NSApplication::sharedApplication(marker);
        let _ = application.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
    });
}

pub fn application_status() -> CapabilityStatus {
    CapabilityStatus::Available
}

pub fn current_application() -> Result<Option<ActiveApplication>, AppError> {
    prepare_background_application();
    let Some(_marker) = MainThreadMarker::new() else {
        return Err(AppError::Unavailable {
            reason: "foreground application lookup must run on the main thread".to_string(),
        });
    };
    let workspace = NSWorkspace::sharedWorkspace();
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
