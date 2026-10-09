//! Shared service identity, error context, and local structured logging.
//!
//! This crate does not load product configuration and does not call operating
//! system capture APIs.

mod capability;
mod error;
mod logging;
mod protocol;
mod service;
mod version;

pub use capability::{
    capability_status_from_permission, combine_health, health_from_capability,
    interpret_settings_launch, permission_copy, user_permission_status, Capability,
    CapabilityRequirement, CapabilityStatus, PermissionCopy, PermissionError, PermissionProvider,
    PermissionState, PermissionStatus, UnknownPermissionProvider, UserPermissionStatus,
};
pub use error::{Error, Result};
pub use logging::{emit, set_log_file, Level, LogFields};
pub use protocol::{
    decode_request, decode_response, encode_request, encode_response, ApplicationMessage,
    HealthMessage, ProcessState, ProtocolError, ReadyMessage, ServiceRequest, ServiceResponse,
    SERVICE_PROTOCOL_VERSION,
};
pub use service::{
    ActivityReport, ApplicationReport, CaptureReport, ExecutionMode, Health, HealthStatus,
    InvalidTransition, PlaceholderService, Readiness, Service, ServiceConfiguration, ServiceId,
    ServiceState, UrlReport,
};
pub use version::{Version, VersionError};

/// Stable technical suffix for the supervisor process.
pub const SUPERVISOR_COMPONENT: &str = "supervisor";

/// Join the product identifier and a stable component suffix.
///
/// Display names are not part of the result.
pub fn technical_id(product_identifier: &str, component: &str) -> String {
    format!("{product_identifier}.{component}")
}

#[cfg(test)]
mod tests {
    use super::{technical_id, ServiceId, SUPERVISOR_COMPONENT};

    #[test]
    fn service_ids_and_technical_suffixes_are_stable() {
        assert_eq!(
            ServiceId::ALL.map(ServiceId::as_str),
            [
                "screenshot",
                "activity",
                "app_tracker",
                "url_tracker",
                "video"
            ]
        );
        assert_eq!(ServiceId::AppTracker.technical_component(), "app-tracker");
        assert_eq!(ServiceId::UrlTracker.technical_component(), "url-tracker");
        assert_eq!(
            technical_id(
                "com.example.agent",
                ServiceId::Screenshot.technical_component()
            ),
            "com.example.agent.screenshot"
        );
        assert_eq!(
            technical_id("com.example.agent", SUPERVISOR_COMPONENT),
            "com.example.agent.supervisor"
        );
    }

    #[test]
    fn technical_components_do_not_collide() {
        let mut components = ServiceId::ALL.map(ServiceId::technical_component).to_vec();
        components.push(SUPERVISOR_COMPONENT);
        let mut unique = components.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), components.len());
    }
}
