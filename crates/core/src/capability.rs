//! Capability and permission contracts.
//!
//! These types describe what a service will need later. They do not capture
//! data, request a permission, or bypass an operating-system control.

use std::fmt;

/// Stable capability identity. The string form is not a display name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Capability {
    ScreenCapture,
    InputMonitoring,
    ActiveApplication,
    BrowserUrl,
    VideoCapture,
}

impl Capability {
    pub const ALL: [Capability; 5] = [
        Self::ScreenCapture,
        Self::InputMonitoring,
        Self::ActiveApplication,
        Self::BrowserUrl,
        Self::VideoCapture,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ScreenCapture => "screen_capture",
            Self::InputMonitoring => "input_monitoring",
            Self::ActiveApplication => "active_application",
            Self::BrowserUrl => "browser_url",
            Self::VideoCapture => "video_capture",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|capability| capability.as_str() == value)
    }

    /// Desktop-session capabilities. Browser URL is queried from the foreground
    /// accessibility tree and is not adjusted by this session flag.
    pub const fn needs_interactive_session(self) -> bool {
        matches!(
            self,
            Self::ScreenCapture
                | Self::InputMonitoring
                | Self::ActiveApplication
                | Self::VideoCapture
        )
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Operating-system permission state. Negative states stay distinct.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PermissionState {
    Granted,
    Denied,
    NotDetermined,
    Restricted,
    Unsupported,
    Unknown,
}

impl PermissionState {
    pub const ALL: [PermissionState; 6] = [
        Self::Granted,
        Self::Denied,
        Self::NotDetermined,
        Self::Restricted,
        Self::Unsupported,
        Self::Unknown,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Granted => "Granted",
            Self::Denied => "Denied",
            Self::NotDetermined => "NotDetermined",
            Self::Restricted => "Restricted",
            Self::Unsupported => "Unsupported",
            Self::Unknown => "Unknown",
        }
    }
}

impl fmt::Display for PermissionState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Whether a service can use a capability. This is not a lifecycle state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CapabilityStatus {
    Available,
    PermissionRequired,
    Unsupported,
    Unavailable,
    Unknown,
}

impl CapabilityStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "Available",
            Self::PermissionRequired => "PermissionRequired",
            Self::Unsupported => "Unsupported",
            Self::Unavailable => "Unavailable",
            Self::Unknown => "Unknown",
        }
    }
}

impl fmt::Display for CapabilityStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One capability a service declares. `required` is false for an optional one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapabilityRequirement {
    pub capability: Capability,
    pub required: bool,
}

/// Result of asking a platform whether a capability is permitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionStatus {
    pub capability: Capability,
    pub state: PermissionState,
    pub reason: Option<String>,
}

impl PermissionStatus {
    pub fn new(capability: Capability, state: PermissionState, reason: impl Into<String>) -> Self {
        Self {
            capability,
            state,
            reason: Some(reason.into()),
        }
    }
}

/// Failure from an explicit settings request. This is not a grant or a bypass.
///
/// `Ok(())` from [`PermissionProvider::open_settings`] means the settings
/// surface was opened. It does not mean the user granted anything.
#[derive(Debug)]
pub enum PermissionError {
    Unsupported {
        capability: Capability,
        reason: String,
    },
    Unavailable {
        capability: Capability,
        reason: String,
    },
    Failed {
        capability: Capability,
        reason: String,
    },
}

impl fmt::Display for PermissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { capability, reason } => {
                write!(
                    formatter,
                    "settings for {capability} are unsupported: {reason}"
                )
            }
            Self::Unavailable { capability, reason } => {
                write!(
                    formatter,
                    "settings for {capability} are unavailable: {reason}"
                )
            }
            Self::Failed { capability, reason } => {
                write!(
                    formatter,
                    "settings for {capability} could not be opened: {reason}"
                )
            }
        }
    }
}

impl PermissionError {
    pub const fn outcome(&self) -> &'static str {
        match self {
            Self::Unsupported { .. } => "Unsupported",
            Self::Unavailable { .. } => "Unavailable",
            Self::Failed { .. } => "Failed",
        }
    }
}

/// What a capability is used for, and what Wind does not collect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PermissionCopy {
    pub title: &'static str,
    pub requirement: &'static str,
    pub privacy: &'static str,
}

pub fn permission_copy(capability: Capability) -> PermissionCopy {
    match capability {
        Capability::ScreenCapture => PermissionCopy {
            title: "Screen Capture",
            requirement: "Wind needs screen-capture access to take screenshots.",
            privacy: "A screenshot is taken only when capture is requested.",
        },
        Capability::VideoCapture => PermissionCopy {
            title: "Video Capture",
            requirement: "Wind needs screen-capture access to record video.",
            privacy: "Video is recorded only when a recording is requested.",
        },
        Capability::InputMonitoring => PermissionCopy {
            title: "Activity Monitoring",
            requirement: "Wind needs input-monitoring access to count keyboard and mouse activity.",
            privacy: "Wind records aggregate keyboard and mouse activity counts. It does not store the characters you type.",
        },
        Capability::ActiveApplication => PermissionCopy {
            title: "Application Tracking",
            requirement: "Wind needs access to the foreground application identity.",
            privacy: "Wind records that identity. It does not read window titles or process memory.",
        },
        Capability::BrowserUrl => PermissionCopy {
            title: "Browser URL",
            requirement: "Wind needs Accessibility access to inspect the browser's address field for URL tracking.",
            privacy: "Wind reads the browser's exposed address field for supported browsers. It does not read browser history, cookies, or page content.",
        },
    }
}

/// User-facing status. This does not replace [`PermissionState`] or [`CapabilityStatus`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserPermissionStatus {
    Enabled,
    PermissionRequired,
    Denied,
    Restricted,
    NotSupported,
    TemporarilyUnavailable,
    Checking,
    Available,
}

impl UserPermissionStatus {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Enabled => "Enabled",
            Self::PermissionRequired => "Permission required",
            Self::Denied => "Permission denied",
            Self::Restricted => "Restricted by the system",
            Self::NotSupported => "Not supported",
            Self::TemporarilyUnavailable => "Temporarily unavailable",
            Self::Checking => "Checking...",
            Self::Available => "Available",
        }
    }

    /// Whether an Open Settings action is meaningful for this status.
    ///
    /// The platform must also have a real settings surface.
    pub const fn offers_settings(self) -> bool {
        matches!(
            self,
            Self::PermissionRequired | Self::Denied | Self::Restricted
        )
    }
}

/// Combine a real permission answer with the current capability view.
///
/// `Unknown` permission is not reported as granted. When the capability is
/// already usable, the status is `Available` instead of a permission prompt.
pub fn user_permission_status(
    permission: PermissionState,
    capability: CapabilityStatus,
) -> UserPermissionStatus {
    match permission {
        PermissionState::Granted => UserPermissionStatus::Enabled,
        PermissionState::NotDetermined => UserPermissionStatus::PermissionRequired,
        PermissionState::Denied => UserPermissionStatus::Denied,
        PermissionState::Restricted => UserPermissionStatus::Restricted,
        PermissionState::Unsupported => UserPermissionStatus::NotSupported,
        PermissionState::Unknown => match capability {
            CapabilityStatus::Available => UserPermissionStatus::Available,
            CapabilityStatus::PermissionRequired => UserPermissionStatus::PermissionRequired,
            CapabilityStatus::Unsupported => UserPermissionStatus::NotSupported,
            CapabilityStatus::Unavailable => UserPermissionStatus::TemporarilyUnavailable,
            CapabilityStatus::Unknown => UserPermissionStatus::Checking,
        },
    }
}

/// A settings process exited or failed to start. `true` means it was opened.
pub fn interpret_settings_launch(
    capability: Capability,
    launched: bool,
) -> Result<(), PermissionError> {
    if launched {
        Ok(())
    } else {
        Err(PermissionError::Failed {
            capability,
            reason: "the settings application could not be opened".to_string(),
        })
    }
}

impl std::error::Error for PermissionError {}

/// Platform-neutral permission lookup.
///
/// Implementations may report state. They must not grant, bypass, or elevate.
pub trait PermissionProvider: Send + Sync {
    fn check(&self, capability: Capability) -> PermissionStatus;

    /// Open the operating-system settings for `capability` when a supported
    /// destination exists. Callers must invoke this explicitly.
    ///
    /// `Ok(())` means the surface was opened. Call [`Self::check`] again.
    /// Opening settings does not grant permission.
    fn open_settings(&self, capability: Capability) -> Result<(), PermissionError>;

    /// Whether `open_settings` has a legitimate destination. This does not open it.
    fn can_open_settings(&self, _capability: Capability) -> bool {
        false
    }

    /// Capability view of [`Self::check`]. Platforms may override this when
    /// the session makes a capability unavailable without a permission answer.
    fn capability_status(&self, capability: Capability) -> CapabilityStatus {
        capability_status_from_permission(self.check(capability).state)
    }
}

/// Map a permission answer onto capability status.
pub fn capability_status_from_permission(state: PermissionState) -> CapabilityStatus {
    match state {
        PermissionState::Granted => CapabilityStatus::Available,
        PermissionState::Denied | PermissionState::NotDetermined | PermissionState::Restricted => {
            CapabilityStatus::PermissionRequired
        }
        PermissionState::Unsupported => CapabilityStatus::Unsupported,
        PermissionState::Unknown => CapabilityStatus::Unknown,
    }
}

/// Health implied by one capability requirement.
///
/// `Unknown` stays unknown. A caller that already has a running placeholder
/// health combines the two with [`combine_health`] so an unknown permission
/// does not erase a healthy lifecycle.
pub fn health_from_capability(required: bool, status: CapabilityStatus) -> super::HealthStatus {
    use super::HealthStatus;
    match (required, status) {
        (_, CapabilityStatus::Available) => HealthStatus::Healthy,
        (_, CapabilityStatus::Unknown) => HealthStatus::Unknown,
        (true, CapabilityStatus::PermissionRequired) => HealthStatus::PermissionRequired,
        (true, CapabilityStatus::Unsupported) => HealthStatus::Unsupported,
        (true, CapabilityStatus::Unavailable) => HealthStatus::Degraded,
        (false, CapabilityStatus::PermissionRequired)
        | (false, CapabilityStatus::Unsupported)
        | (false, CapabilityStatus::Unavailable) => HealthStatus::Degraded,
    }
}

/// Combine service health with capability health.
///
/// An actual service failure wins. An unknown or available capability leaves
/// the service health in place. A determined permission or support problem
/// replaces a non-failure health and does not invent a `Failed` lifecycle.
pub fn combine_health(
    service_health: super::HealthStatus,
    capability_health: super::HealthStatus,
) -> super::HealthStatus {
    use super::HealthStatus;
    if service_health == HealthStatus::Unhealthy {
        return service_health;
    }
    match capability_health {
        HealthStatus::Healthy | HealthStatus::Unknown => service_health,
        other => other,
    }
}

/// Provider used when no platform provider has been installed.
///
/// Every capability is `Unknown`. Settings are not opened.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnknownPermissionProvider;

impl PermissionProvider for UnknownPermissionProvider {
    fn check(&self, capability: Capability) -> PermissionStatus {
        PermissionStatus::new(
            capability,
            PermissionState::Unknown,
            "permission status has not been evaluated",
        )
    }

    fn open_settings(&self, capability: Capability) -> Result<(), PermissionError> {
        Err(PermissionError::Unsupported {
            capability,
            reason: "no settings destination is available".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        capability_status_from_permission, combine_health, health_from_capability, Capability,
        CapabilityStatus, PermissionState,
    };
    use crate::HealthStatus;

    #[test]
    fn capability_identifiers_are_stable() {
        assert_eq!(
            Capability::ALL.map(Capability::as_str),
            [
                "screen_capture",
                "input_monitoring",
                "active_application",
                "browser_url",
                "video_capture"
            ]
        );
    }

    #[test]
    fn permission_states_stay_distinct() {
        let labels: Vec<_> = PermissionState::ALL
            .into_iter()
            .map(PermissionState::as_str)
            .collect();
        assert_eq!(
            labels,
            [
                "Granted",
                "Denied",
                "NotDetermined",
                "Restricted",
                "Unsupported",
                "Unknown"
            ]
        );
        assert_eq!(labels.len(), 6);
    }

    #[test]
    fn permission_state_maps_to_capability_status() {
        assert_eq!(
            capability_status_from_permission(PermissionState::Granted),
            CapabilityStatus::Available
        );
        assert_eq!(
            capability_status_from_permission(PermissionState::Denied),
            CapabilityStatus::PermissionRequired
        );
        assert_eq!(
            capability_status_from_permission(PermissionState::NotDetermined),
            CapabilityStatus::PermissionRequired
        );
        assert_eq!(
            capability_status_from_permission(PermissionState::Restricted),
            CapabilityStatus::PermissionRequired
        );
        assert_eq!(
            capability_status_from_permission(PermissionState::Unsupported),
            CapabilityStatus::Unsupported
        );
        assert_eq!(
            capability_status_from_permission(PermissionState::Unknown),
            CapabilityStatus::Unknown
        );
    }

    #[test]
    fn required_and_optional_capabilities_change_health_without_a_failure() {
        assert_eq!(
            health_from_capability(true, CapabilityStatus::Available),
            HealthStatus::Healthy
        );
        assert_eq!(
            health_from_capability(false, CapabilityStatus::Available),
            HealthStatus::Healthy
        );
        assert_eq!(
            health_from_capability(true, CapabilityStatus::PermissionRequired),
            HealthStatus::PermissionRequired
        );
        assert_eq!(
            health_from_capability(true, CapabilityStatus::Unavailable),
            HealthStatus::Degraded
        );
        assert_eq!(
            health_from_capability(false, CapabilityStatus::Unavailable),
            HealthStatus::Degraded
        );
        assert_eq!(
            health_from_capability(false, CapabilityStatus::PermissionRequired),
            HealthStatus::Degraded
        );
        assert_eq!(
            health_from_capability(true, CapabilityStatus::Unsupported),
            HealthStatus::Unsupported
        );
        assert_ne!(
            health_from_capability(true, CapabilityStatus::PermissionRequired),
            HealthStatus::Unhealthy
        );
    }

    #[test]
    fn unknown_permission_does_not_replace_running_health() {
        assert_eq!(
            combine_health(
                HealthStatus::Healthy,
                health_from_capability(true, CapabilityStatus::Unknown)
            ),
            HealthStatus::Healthy
        );
        assert_eq!(
            combine_health(
                HealthStatus::Healthy,
                health_from_capability(true, CapabilityStatus::PermissionRequired)
            ),
            HealthStatus::PermissionRequired
        );
        assert_eq!(
            combine_health(
                HealthStatus::Unhealthy,
                health_from_capability(true, CapabilityStatus::PermissionRequired)
            ),
            HealthStatus::Unhealthy
        );
    }

    #[test]
    fn user_status_keeps_permission_states_distinct() {
        use super::{user_permission_status, UserPermissionStatus};
        assert_eq!(
            user_permission_status(PermissionState::Granted, CapabilityStatus::Available),
            UserPermissionStatus::Enabled
        );
        assert_eq!(
            user_permission_status(
                PermissionState::Denied,
                CapabilityStatus::PermissionRequired
            ),
            UserPermissionStatus::Denied
        );
        assert_eq!(
            user_permission_status(
                PermissionState::NotDetermined,
                CapabilityStatus::PermissionRequired
            ),
            UserPermissionStatus::PermissionRequired
        );
        assert_eq!(
            user_permission_status(
                PermissionState::Restricted,
                CapabilityStatus::PermissionRequired
            ),
            UserPermissionStatus::Restricted
        );
        assert_eq!(
            user_permission_status(PermissionState::Unsupported, CapabilityStatus::Unsupported),
            UserPermissionStatus::NotSupported
        );
        assert_eq!(
            user_permission_status(PermissionState::Unknown, CapabilityStatus::Unknown),
            UserPermissionStatus::Checking
        );
        assert_eq!(
            user_permission_status(PermissionState::Unknown, CapabilityStatus::Available),
            UserPermissionStatus::Available
        );
        assert_eq!(
            user_permission_status(PermissionState::Unknown, CapabilityStatus::Unavailable),
            UserPermissionStatus::TemporarilyUnavailable
        );
        assert!(!UserPermissionStatus::Available.offers_settings());
        assert!(!UserPermissionStatus::Checking.offers_settings());
        assert!(!UserPermissionStatus::NotSupported.offers_settings());
        assert!(UserPermissionStatus::PermissionRequired.offers_settings());
        assert!(UserPermissionStatus::Denied.offers_settings());
    }

    #[test]
    fn a_failed_settings_launch_is_a_controlled_error() {
        use super::interpret_settings_launch;
        assert!(interpret_settings_launch(Capability::ScreenCapture, true).is_ok());
        let error = interpret_settings_launch(Capability::ScreenCapture, false).unwrap_err();
        assert_eq!(error.outcome(), "Failed");
        assert!(error.to_string().contains("could not be opened"));
    }

    #[test]
    fn guidance_describes_activity_and_url_limits() {
        use super::permission_copy;
        let activity = permission_copy(Capability::InputMonitoring);
        assert!(activity.privacy.contains("does not store the characters"));
        let url = permission_copy(Capability::BrowserUrl);
        assert!(url.privacy.contains("does not read browser history"));
        assert!(url.requirement.contains("address field"));
    }
}
