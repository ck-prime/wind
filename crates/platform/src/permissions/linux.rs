//! Linux permission answers.
//!
//! The permission provider does not open a portal or grant access. A missing
//! interactive session marks a desktop capability unavailable. The capture
//! provider reports whether an X11 or Wayland path is actually usable.

use crate::SessionKind;
use wind_core::{Capability, CapabilityStatus, PermissionError, PermissionState, PermissionStatus};

pub fn permission(capability: Capability, session: SessionKind) -> PermissionStatus {
    if capability.needs_interactive_session() && session != SessionKind::Interactive {
        return PermissionStatus::new(
            capability,
            PermissionState::Unknown,
            "no interactive desktop session is available; desktop permissions are not queried",
        );
    }
    let reason = match capability {
        Capability::BrowserUrl => {
            "AT-SPI access is reported by the URL provider and is not requested"
        }
        Capability::ScreenCapture => {
            "Linux screen capture is not a single consent query; the provider reports X11 or portal usability"
        }
        Capability::InputMonitoring => "Linux input monitoring permission is not queried",
        Capability::ActiveApplication => "Linux foreground application permission is not queried",
        Capability::VideoCapture => {
            "Linux video capture is not a single consent query; the provider reports X11 or portal usability"
        }
    };
    PermissionStatus::new(capability, PermissionState::Unknown, reason)
}

/// Linux has no single settings page for these capabilities.
///
/// A headless or non-interactive session is `Unavailable`. An X11 or Wayland
/// session is `Unsupported` because opening a desktop-specific control, or the
/// screen-capture portal, would be the wrong surface. This does not spawn a
/// process.
#[cfg_attr(not(any(test, target_os = "linux")), allow(dead_code))]
pub fn open_settings(
    capability: Capability,
    interactive: bool,
    x11: bool,
    wayland: bool,
) -> Result<(), PermissionError> {
    if !interactive || (!x11 && !wayland) {
        return Err(PermissionError::Unavailable {
            capability,
            reason: "no interactive desktop session is available".to_string(),
        });
    }
    let reason = match (capability, wayland) {
        (Capability::ScreenCapture | Capability::VideoCapture, true) => {
            "Wayland screen capture uses the desktop portal when a capture is requested. Wind does not open that dialog from settings and does not bypass the compositor."
        }
        (Capability::ScreenCapture | Capability::VideoCapture, false) => {
            "X11 does not provide a user-grantable settings page for screen capture."
        }
        (Capability::InputMonitoring, true) => {
            "Wayland does not provide a global input-monitoring settings page that Wind can open."
        }
        (Capability::InputMonitoring, false) => {
            "X11 does not provide a user-grantable settings page for input monitoring."
        }
        (Capability::BrowserUrl, _) => {
            "Linux does not provide one accessibility settings page for every desktop. Wind does not bypass AT-SPI."
        }
        (Capability::ActiveApplication, _) => {
            "Foreground application identity has no separate Linux settings page."
        }
    };
    Err(PermissionError::Unsupported {
        capability,
        reason: reason.to_string(),
    })
}

/// Session adjustment for a permission-derived capability status.
///
/// A non-interactive session makes a desktop capability unavailable when the
/// permission answer is still unknown or already granted. A denial or an
/// unsupported answer is left unchanged. Browser URL is not adjusted.
pub fn apply_session(
    status: CapabilityStatus,
    capability: Capability,
    session: SessionKind,
) -> CapabilityStatus {
    if capability.needs_interactive_session()
        && session != SessionKind::Interactive
        && matches!(
            status,
            CapabilityStatus::Unknown | CapabilityStatus::Available
        )
    {
        CapabilityStatus::Unavailable
    } else {
        status
    }
}
