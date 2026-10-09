//! macOS permission answers that do not query TCC.
//!
//! Screen capture on a macOS build is answered by `CGPreflightScreenCaptureAccess`
//! in the host provider. This function remains the non-querying fallback for
//! the other capabilities and for tests of the static table. It does not prompt.
//! Settings URLs are selected here and opened only by an explicit request.

use wind_core::{Capability, PermissionError, PermissionState, PermissionStatus};

/// Direct System Settings destination, when one matches the capability.
///
/// These `Privacy_*` anchors are the long-standing System Settings destinations.
/// Active application identity has no privacy pane. The URL is not opened here.
#[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
pub fn settings_url(capability: Capability) -> Option<&'static str> {
    match capability {
        Capability::ScreenCapture | Capability::VideoCapture => {
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        }
        Capability::InputMonitoring => {
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
        }
        Capability::BrowserUrl => {
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        }
        Capability::ActiveApplication => None,
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn open_settings(capability: Capability) -> Result<(), PermissionError> {
    let Some(_url) = settings_url(capability) else {
        return Err(PermissionError::Unsupported {
            capability,
            reason: "this capability has no macOS privacy setting".to_string(),
        });
    };
    #[cfg(target_os = "macos")]
    {
        return wind_macos::open_system_settings(_url).map_err(|_| PermissionError::Failed {
            capability,
            reason: "System Settings could not be opened".to_string(),
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(PermissionError::Unavailable {
            capability,
            reason: "System Settings can be opened only on macOS".to_string(),
        })
    }
}

pub fn permission(capability: Capability) -> PermissionStatus {
    match capability {
        Capability::ActiveApplication => PermissionStatus::new(
            capability,
            PermissionState::Granted,
            "foreground application identity does not require a separate privacy permission",
        ),
        Capability::BrowserUrl => PermissionStatus::new(
            capability,
            PermissionState::Unknown,
            "Accessibility permission is checked by the URL provider and is not requested here",
        ),
        Capability::ScreenCapture => PermissionStatus::new(
            capability,
            PermissionState::Unknown,
            "screen capture readiness is read by the macOS host provider; this fallback does not query it",
        ),
        Capability::InputMonitoring => PermissionStatus::new(
            capability,
            PermissionState::Unknown,
            "macOS input monitoring permission is not queried and is not requested",
        ),
        Capability::VideoCapture => PermissionStatus::new(
            capability,
            PermissionState::Unknown,
            "macOS screen capture permission is not queried and is not requested",
        ),
    }
}
