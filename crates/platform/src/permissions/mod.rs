//! Permission providers.
//!
//! Platform modules answer capability questions. They do not capture data,
//! prompt the user, or bypass an operating-system control. Settings open only
//! when `open_settings` is called and a real destination exists.

mod linux;
mod macos;
mod windows;

use wind_core::{
    capability_status_from_permission, Capability, CapabilityStatus, PermissionError,
    PermissionProvider, PermissionStatus,
};

pub use linux::apply_session;
pub use linux::permission as linux_permission;
pub use macos::permission as macos_permission;
pub use windows::permission as windows_permission;

use crate::SessionKind;

/// Permission provider for the operating system this binary was built for.
#[derive(Debug, Clone, Copy)]
pub struct HostPermissionProvider {
    session: SessionKind,
}

impl HostPermissionProvider {
    pub fn new(session: SessionKind) -> Self {
        Self { session }
    }
}

impl PermissionProvider for HostPermissionProvider {
    fn check(&self, capability: Capability) -> PermissionStatus {
        #[cfg(target_os = "macos")]
        {
            if capability == Capability::ScreenCapture {
                crate::screenshot::macos::screen_capture_permission()
            } else if capability == Capability::InputMonitoring {
                wind_macos::input_monitoring_permission()
            } else {
                macos::permission(capability)
            }
        }
        #[cfg(target_os = "windows")]
        {
            windows::permission(capability)
        }
        #[cfg(target_os = "linux")]
        {
            linux::permission(capability, self.session)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = (capability, self.session);
            PermissionStatus::new(
                capability,
                wind_core::PermissionState::Unknown,
                "this operating system has no permission provider",
            )
        }
    }

    fn capability_status(&self, capability: Capability) -> CapabilityStatus {
        apply_session(
            capability_status_from_permission(self.check(capability).state),
            capability,
            self.session,
        )
    }

    fn open_settings(&self, capability: Capability) -> Result<(), PermissionError> {
        open_host_settings(self.session, capability)
    }

    fn can_open_settings(&self, capability: Capability) -> bool {
        host_can_open_settings(capability)
    }
}

fn host_can_open_settings(capability: Capability) -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::settings_url(capability).is_some()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = capability;
        false
    }
}

fn open_host_settings(
    session: crate::SessionKind,
    capability: Capability,
) -> Result<(), PermissionError> {
    #[cfg(target_os = "macos")]
    {
        let _ = session;
        macos::open_settings(capability)
    }
    #[cfg(target_os = "windows")]
    {
        let _ = session;
        windows::open_settings(capability)
    }
    #[cfg(target_os = "linux")]
    {
        let interactive = session == crate::SessionKind::Interactive;
        let x11 = std::env::var_os("DISPLAY").is_some();
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        linux::open_settings(capability, interactive, x11, wayland)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = session;
        Err(PermissionError::Unsupported {
            capability,
            reason: "this operating system has no settings destination".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_session, linux_permission, macos_permission, windows_permission,
        HostPermissionProvider,
    };
    use crate::SessionKind;
    use wind_core::{
        capability_status_from_permission, Capability, CapabilityStatus, PermissionProvider,
        PermissionState,
    };

    #[test]
    fn macos_answers_are_explicit_and_do_not_grant_capture() {
        assert_eq!(
            macos_permission(Capability::ActiveApplication).state,
            PermissionState::Granted
        );
        for capability in [
            Capability::ScreenCapture,
            Capability::InputMonitoring,
            Capability::VideoCapture,
            Capability::BrowserUrl,
        ] {
            let status = macos_permission(capability);
            assert_eq!(status.state, PermissionState::Unknown);
            assert_ne!(status.state, PermissionState::Granted);
        }
    }

    #[test]
    fn windows_does_not_invent_granted_permissions() {
        for capability in Capability::ALL {
            let status = windows_permission(capability);
            assert_eq!(status.state, PermissionState::Unknown);
            assert_ne!(status.state, PermissionState::Granted);
            assert!(status.reason.is_some());
        }
    }

    #[test]
    fn linux_non_interactive_session_makes_desktop_capabilities_unavailable() {
        let screen = linux_permission(Capability::ScreenCapture, SessionKind::NonInteractive);
        assert_eq!(screen.state, PermissionState::Unknown);
        assert_eq!(
            apply_session(
                capability_status_from_permission(screen.state),
                Capability::ScreenCapture,
                SessionKind::NonInteractive
            ),
            CapabilityStatus::Unavailable
        );
        let status = linux_permission(Capability::InputMonitoring, SessionKind::NonInteractive);
        assert_eq!(status.state, PermissionState::Unknown);
        assert_eq!(
            apply_session(
                capability_status_from_permission(status.state),
                Capability::InputMonitoring,
                SessionKind::NonInteractive
            ),
            CapabilityStatus::Unavailable
        );
        let browser = linux_permission(Capability::BrowserUrl, SessionKind::NonInteractive);
        assert_eq!(
            apply_session(
                capability_status_from_permission(browser.state),
                Capability::BrowserUrl,
                SessionKind::NonInteractive
            ),
            CapabilityStatus::Unknown
        );
        assert_eq!(
            apply_session(
                CapabilityStatus::PermissionRequired,
                Capability::ScreenCapture,
                SessionKind::NonInteractive
            ),
            CapabilityStatus::PermissionRequired
        );
    }

    #[test]
    fn opening_settings_does_not_grant_permission() {
        let provider = HostPermissionProvider::new(SessionKind::Interactive);
        let before = provider.check(Capability::ScreenCapture).state;
        let opened = provider.open_settings(Capability::ScreenCapture);
        let after = provider.check(Capability::ScreenCapture).state;
        assert_eq!(before, after);
        assert_ne!(after, PermissionState::Granted);
        #[cfg(target_os = "windows")]
        {
            assert!(opened.is_err());
            assert!(!provider.can_open_settings(Capability::ScreenCapture));
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = opened;
        }
    }

    #[test]
    fn windows_settings_are_unsupported_for_every_capability() {
        for capability in Capability::ALL {
            let error = windows_permission_settings(capability);
            let again = windows_permission_settings(capability);
            assert_eq!(error.outcome(), "Unsupported");
            assert_eq!(again.outcome(), "Unsupported");
            assert!(!error.to_string().contains("Grant"));
        }
    }

    #[test]
    fn macos_settings_match_the_real_privacy_panes() {
        use super::macos::settings_url;
        assert!(settings_url(Capability::ScreenCapture)
            .unwrap()
            .contains("Privacy_ScreenCapture"));
        assert!(settings_url(Capability::VideoCapture)
            .unwrap()
            .contains("Privacy_ScreenCapture"));
        assert!(settings_url(Capability::InputMonitoring)
            .unwrap()
            .contains("Privacy_ListenEvent"));
        assert!(settings_url(Capability::BrowserUrl)
            .unwrap()
            .contains("Privacy_Accessibility"));
        assert!(settings_url(Capability::ActiveApplication).is_none());
    }

    #[test]
    fn linux_settings_follow_the_desktop_environment() {
        use super::linux::open_settings;
        let headless = open_settings(Capability::ScreenCapture, false, false, false).unwrap_err();
        assert_eq!(headless.outcome(), "Unavailable");
        let wayland = open_settings(Capability::ScreenCapture, true, false, true).unwrap_err();
        assert_eq!(wayland.outcome(), "Unsupported");
        assert!(wayland.to_string().contains("portal") || wayland.to_string().contains("Wayland"));
        let x11 = open_settings(Capability::InputMonitoring, true, true, false).unwrap_err();
        assert_eq!(x11.outcome(), "Unsupported");
        let url = open_settings(Capability::BrowserUrl, true, true, false).unwrap_err();
        assert_eq!(url.outcome(), "Unsupported");
    }

    fn windows_permission_settings(capability: Capability) -> wind_core::PermissionError {
        super::windows::open_settings(capability).expect_err("windows has no settings page")
    }
}
