//! Windows permission answers.
//!
//! Windows does not expose a normal-user consent query for these capabilities.
//! This module does not invent `Granted` and does not open a settings page.

use wind_core::{Capability, PermissionError, PermissionState, PermissionStatus};

pub fn permission(capability: Capability) -> PermissionStatus {
    PermissionStatus::new(capability, PermissionState::Unknown, reason(capability))
}

pub fn reason(capability: Capability) -> &'static str {
    match capability {
        Capability::ScreenCapture => {
            "Windows does not use a separate screen-capture permission. Wind uses desktop capture when the session can provide it."
        }
        Capability::VideoCapture => {
            "Windows does not use a separate video-capture permission. Wind uses desktop capture when a recording is requested."
        }
        Capability::InputMonitoring => {
            "Windows does not use a separate keyboard and mouse permission for a normal user. Wind records aggregate activity counts and does not store typed characters."
        }
        Capability::ActiveApplication => {
            "Windows does not use a separate permission for foreground application identity."
        }
        Capability::BrowserUrl => {
            "Wind cannot access an application's accessibility information when a Windows security boundary blocks it. There is no user-grantable browser permission to open."
        }
    }
}

/// Windows has no legitimate settings page for these capabilities.
#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
pub fn open_settings(capability: Capability) -> Result<(), PermissionError> {
    Err(PermissionError::Unsupported {
        capability,
        reason: reason(capability).to_string(),
    })
}
