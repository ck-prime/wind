use std::fmt;
use std::time::SystemTime;

use crate::capability::CapabilityRequirement;
use crate::version::Version;
use crate::Result;

/// Stable service identity.
///
/// The string form is the registration key. It does not include the product
/// display name. `technical_component` is the suffix appended to the product
/// identifier for signing, discovery, and installation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ServiceId {
    Screenshot,
    Activity,
    AppTracker,
    UrlTracker,
    Video,
}

impl ServiceId {
    pub const ALL: [ServiceId; 5] = [
        ServiceId::Screenshot,
        ServiceId::Activity,
        ServiceId::AppTracker,
        ServiceId::UrlTracker,
        ServiceId::Video,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Screenshot => "screenshot",
            Self::Activity => "activity",
            Self::AppTracker => "app_tracker",
            Self::UrlTracker => "url_tracker",
            Self::Video => "video",
        }
    }

    pub const fn technical_component(self) -> &'static str {
        match self {
            Self::Screenshot => "screenshot",
            Self::Activity => "activity",
            Self::AppTracker => "app-tracker",
            Self::UrlTracker => "url-tracker",
            Self::Video => "video",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Screenshot => "Screenshot",
            Self::Activity => "Activity",
            Self::AppTracker => "App Tracker",
            Self::UrlTracker => "URL Tracker",
            Self::Video => "Video",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|service| service.as_str() == value)
    }
}

/// How a service will run. Phase 2 only constructs in-process services.
///
/// `OutOfProcess` exists so a later handle can describe a separate process
/// without changing the supervisor's lifecycle API. The service manager does
/// not spawn processes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionMode {
    InProcess,
    OutOfProcess,
}

impl ExecutionMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InProcess => "in-process",
            Self::OutOfProcess => "out-of-process",
        }
    }
}

/// Runtime lifecycle. The service manager is the only component that changes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState {
    Registered,
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
    Restarting,
    Disabled,
}

impl ServiceState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Registered => "Registered",
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Stopping => "Stopping",
            Self::Stopped => "Stopped",
            Self::Failed => "Failed",
            Self::Restarting => "Restarting",
            Self::Disabled => "Disabled",
        }
    }

    /// Move to `to` only when the pair is a documented transition.
    pub fn transition(self, to: Self) -> std::result::Result<Self, InvalidTransition> {
        let allowed = matches!(
            (self, to),
            (Self::Registered, Self::Starting)
                | (Self::Registered, Self::Disabled)
                | (Self::Starting, Self::Running)
                | (Self::Starting, Self::Failed)
                | (Self::Running, Self::Stopping)
                | (Self::Running, Self::Failed)
                | (Self::Stopping, Self::Stopped)
                | (Self::Stopping, Self::Failed)
                | (Self::Stopped, Self::Starting)
                | (Self::Stopped, Self::Disabled)
                | (Self::Stopped, Self::Restarting)
                | (Self::Failed, Self::Restarting)
                | (Self::Restarting, Self::Starting)
                | (Self::Restarting, Self::Failed)
        );
        if allowed {
            Ok(to)
        } else {
            Err(InvalidTransition { from: self, to })
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidTransition {
    pub from: ServiceState,
    pub to: ServiceState,
}

impl std::fmt::Display for InvalidTransition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "invalid transition from {} to {}",
            self.from.as_str(),
            self.to.as_str()
        )
    }
}

impl std::error::Error for InvalidTransition {}

/// Health reported for a service. It is not a lifecycle state.
///
/// `PermissionRequired` and `Unsupported` describe capability readiness. They
/// are not `Failed`. `Unhealthy` is reserved for an actual service failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
    PermissionRequired,
    Unsupported,
}

impl HealthStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "Healthy",
            Self::Degraded => "Degraded",
            Self::Unhealthy => "Unhealthy",
            Self::Unknown => "Unknown",
            Self::PermissionRequired => "PermissionRequired",
            Self::Unsupported => "Unsupported",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Health {
    pub status: HealthStatus,
    pub detail: &'static str,
}

/// Placeholder for a future permission check. Phase 2 does not query the OS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    Unknown,
    Ready,
    PermissionRequired,
}

impl Readiness {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::Ready => "Ready",
            Self::PermissionRequired => "PermissionRequired",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceConfiguration {
    pub id: ServiceId,
    pub version: Version,
    /// Capability collection stays off until a later phase designs it.
    pub monitoring_enabled: bool,
}

/// Lifecycle surface every capability service exposes.
///
/// The service manager calls this trait for both today's in-process
/// placeholders and a future process-backed handle. Implementations do not
/// change manager state.
pub trait Service: Send {
    fn id(&self) -> ServiceId;
    fn name(&self) -> &str;
    fn version(&self) -> Version;
    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::InProcess
    }
    fn start(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn health(&self) -> Health;
    fn readiness(&self) -> Readiness {
        Readiness::Unknown
    }
    /// Capabilities this service requires. An empty slice means none.
    fn capabilities(&self) -> &'static [CapabilityRequirement] {
        &[]
    }
    fn configuration(&self) -> ServiceConfiguration;

    /// Service wire protocol. In-process and out-of-process services share it.
    fn protocol_version(&self) -> u32 {
        crate::SERVICE_PROTOCOL_VERSION
    }

    /// Read cumulative activity without writing a storage row.
    fn observe_activity(&mut self) -> Result<ActivityReport> {
        Err(crate::Error::new(
            self.id().as_str(),
            "observe_activity",
            "this service does not track activity",
        )
        .with_service(self.id().as_str()))
    }

    /// Read the foreground application without writing a storage row.
    fn observe_application(&mut self) -> Result<ApplicationReport> {
        Err(crate::Error::new(
            self.id().as_str(),
            "observe_application",
            "this service does not track applications",
        )
        .with_service(self.id().as_str()))
    }

    /// Read the current URL without writing a storage row.
    fn observe_url(&mut self) -> Result<UrlReport> {
        Err(crate::Error::new(
            self.id().as_str(),
            "observe_url",
            "this service does not track urls",
        )
        .with_service(self.id().as_str()))
    }

    /// Capture one screenshot file without registering a separate interval.
    fn capture_interval_screenshot(&mut self) -> Result<CaptureReport> {
        Err(crate::Error::new(
            self.id().as_str(),
            "capture_interval_screenshot",
            "this service does not capture screenshots",
        )
        .with_service(self.id().as_str()))
    }

    /// The interval engine owns finalized rows, so stop must not write them again.
    fn release_stop_flush(&mut self) {}

    /// Supervisor-owned recovery hook. The default service has nothing to restart.
    ///
    /// A crash loop returns an error. The manager can then mark that service
    /// failed without stopping the supervisor.
    fn maintenance(&mut self) -> Result<()> {
        Ok(())
    }

    /// Capture one screenshot. The default service has no capture path.
    fn capture_screenshot(&mut self) -> Result<CaptureReport> {
        Err(crate::Error::new(
            self.id().as_str(),
            "capture_screenshot",
            "this service does not capture screenshots",
        )
        .with_service(self.id().as_str()))
    }

    /// Start one video recording. The default service has no recording path.
    fn start_recording(&mut self) -> Result<()> {
        Err(crate::Error::new(
            self.id().as_str(),
            "start_recording",
            "this service does not record video",
        )
        .with_service(self.id().as_str()))
    }

    /// Stop the current video recording and return the stored artifact.
    fn stop_recording(&mut self) -> Result<CaptureReport> {
        Err(crate::Error::new(
            self.id().as_str(),
            "stop_recording",
            "this service does not record video",
        )
        .with_service(self.id().as_str()))
    }

    /// Read the foreground application. The default service has no tracker.
    fn current_application(&mut self) -> Result<ApplicationReport> {
        Err(crate::Error::new(
            self.id().as_str(),
            "current_application",
            "this service does not track applications",
        )
        .with_service(self.id().as_str()))
    }

    /// Read the latest browser URL observation. The default service has none.
    fn current_url(&mut self) -> Result<UrlReport> {
        Err(crate::Error::new(
            self.id().as_str(),
            "current_url",
            "this service does not track urls",
        )
        .with_service(self.id().as_str()))
    }

    /// Read cumulative input counters. The default service has no activity path.
    fn activity_snapshot(&mut self) -> Result<ActivityReport> {
        Err(crate::Error::new(
            self.id().as_str(),
            "activity_snapshot",
            "this service does not track activity",
        )
        .with_service(self.id().as_str()))
    }
}

/// Result of an explicit screenshot or video command.
///
/// `artifact` is the local file path for the caller that requested the
/// command. Service logs do not include it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureReport {
    pub width: u32,
    pub height: u32,
    pub byte_len: u64,
    pub duration_ms: u64,
    pub frames: u64,
    pub artifact: String,
    pub captured_at: SystemTime,
}

/// Result of an explicit foreground-application query.
///
/// Debug output keeps the application id only when it is not a filesystem
/// path. Names and paths stay on the struct for the caller and are omitted
/// from the debug text.
#[derive(Clone, PartialEq, Eq)]
pub struct ApplicationReport {
    pub application_id: String,
    pub name: String,
    pub executable: String,
    pub present: bool,
    pub changed: bool,
}

impl fmt::Debug for ApplicationReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApplicationReport")
            .field("present", &self.present)
            .field("changed", &self.changed)
            .finish()
    }
}

/// Result of an explicit activity snapshot.
///
/// The fields are aggregate counts. Debug output is the same set of counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityReport {
    pub observed_at: SystemTime,
    pub keyboard_events: u64,
    pub mouse_moves: u64,
    pub mouse_clicks: u64,
    pub mouse_scrolls: u64,
}

/// Result of an explicit URL query.
///
/// `url` is the sanitized value for the caller that asked. Debug output does
/// not include it.
#[derive(Clone, PartialEq, Eq)]
pub struct UrlReport {
    pub browser: String,
    pub url: String,
    pub present: bool,
    pub changed: bool,
}

impl fmt::Debug for UrlReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UrlReport")
            .field("browser", &self.browser)
            .field("present", &self.present)
            .field("changed", &self.changed)
            .finish()
    }
}

/// In-process placeholder used until a capability implements real work.
///
/// `start` records the request. It does not capture screens, input,
/// applications, URLs, or video, and it does not request permissions.
#[derive(Debug)]
pub struct PlaceholderService {
    id: ServiceId,
    version: Version,
    started: bool,
    requirements: &'static [CapabilityRequirement],
}

impl PlaceholderService {
    pub fn new(id: ServiceId, version: Version) -> Self {
        Self {
            id,
            version,
            started: false,
            requirements: &[],
        }
    }

    /// Declare the capabilities this placeholder stands in for.
    ///
    /// Declaration does not enable monitoring and does not request permission.
    pub fn requiring(mut self, requirements: &'static [CapabilityRequirement]) -> Self {
        self.requirements = requirements;
        self
    }

    pub fn from_crate_version(id: ServiceId, crate_version: &str) -> Result<Self> {
        let version = Version::parse(crate_version).map_err(|error| {
            crate::Error::new(id.as_str(), "read_version", error.to_string())
                .with_service(id.as_str())
        })?;
        Ok(Self::new(id, version))
    }
}

impl Service for PlaceholderService {
    fn id(&self) -> ServiceId {
        self.id
    }

    fn name(&self) -> &str {
        self.id.display_name()
    }

    fn version(&self) -> Version {
        self.version
    }

    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::InProcess
    }

    fn start(&mut self) -> Result<()> {
        self.started = true;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.started = false;
        Ok(())
    }

    fn health(&self) -> Health {
        if self.started {
            Health {
                status: HealthStatus::Healthy,
                detail: "placeholder running; monitoring is not implemented",
            }
        } else {
            Health {
                status: HealthStatus::Unknown,
                detail: "placeholder is not running",
            }
        }
    }

    fn readiness(&self) -> Readiness {
        Readiness::Unknown
    }

    fn capabilities(&self) -> &'static [CapabilityRequirement] {
        self.requirements
    }

    fn configuration(&self) -> ServiceConfiguration {
        ServiceConfiguration {
            id: self.id,
            version: self.version,
            monitoring_enabled: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ServiceState;

    #[test]
    fn allows_documented_transitions() {
        let allowed = [
            (ServiceState::Registered, ServiceState::Starting),
            (ServiceState::Registered, ServiceState::Disabled),
            (ServiceState::Starting, ServiceState::Running),
            (ServiceState::Starting, ServiceState::Failed),
            (ServiceState::Running, ServiceState::Stopping),
            (ServiceState::Running, ServiceState::Failed),
            (ServiceState::Stopping, ServiceState::Stopped),
            (ServiceState::Stopping, ServiceState::Failed),
            (ServiceState::Stopped, ServiceState::Starting),
            (ServiceState::Stopped, ServiceState::Disabled),
            (ServiceState::Stopped, ServiceState::Restarting),
            (ServiceState::Failed, ServiceState::Restarting),
            (ServiceState::Restarting, ServiceState::Starting),
            (ServiceState::Restarting, ServiceState::Failed),
        ];
        for (from, to) in allowed {
            assert_eq!(from.transition(to).unwrap(), to);
        }
    }

    #[test]
    fn rejects_invalid_transitions() {
        let rejected = [
            (ServiceState::Running, ServiceState::Registered),
            (ServiceState::Disabled, ServiceState::Starting),
            (ServiceState::Stopped, ServiceState::Running),
            (ServiceState::Failed, ServiceState::Running),
            (ServiceState::Registered, ServiceState::Running),
        ];
        for (from, to) in rejected {
            let error = from.transition(to).unwrap_err();
            assert_eq!(error.from, from);
            assert_eq!(error.to, to);
        }
    }
}
