//! In-process service runtime.
//!
//! Registers services, owns the lifecycle state machine, and shuts them down
//! in reverse registration order. It does not collect monitoring data, spawn
//! processes, elevate privileges, or talk to a backend.

use std::time::{Duration, Instant};

use wind_core::{
    combine_health, emit, health_from_capability, ActivityReport, ApplicationReport, Capability,
    CapabilityStatus, CaptureReport, Error, ExecutionMode, Health, HealthStatus, Level, LogFields,
    PermissionProvider, Readiness, Result, Service, ServiceId, ServiceState,
    UnknownPermissionProvider, UrlReport, Version,
};

/// Budget for one synchronous stop call.
///
/// Phase 2 services return from `stop` on the calling thread. The manager
/// reports an error when that return takes longer than this budget. It does
/// not kill the process. The default matches `[runtime] shutdown_timeout_seconds`.
/// The supervisor passes the configured value instead of relying on this default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSettings {
    pub shutdown_timeout: Duration,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            shutdown_timeout: Duration::from_secs(5),
        }
    }
}

/// Metadata assembled once by the service manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceDescriptor {
    pub id: ServiceId,
    pub display_name: String,
    pub version: Version,
    pub execution_mode: ExecutionMode,
    pub enabled: bool,
}

/// Point-in-time view of one registered service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceStatus {
    pub descriptor: ServiceDescriptor,
    pub state: ServiceState,
    pub health: Health,
    pub readiness: Readiness,
}

/// One capability observation. Lifecycle, capability status, and permission
/// state stay independent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityObservation {
    pub service_id: ServiceId,
    pub lifecycle: ServiceState,
    pub health: HealthStatus,
    pub capability: Capability,
    pub required: bool,
    pub capability_status: CapabilityStatus,
    pub permission: wind_core::PermissionState,
    pub reason: Option<String>,
}

struct Entry {
    service: Box<dyn Service>,
    state: ServiceState,
    enabled: bool,
}

/// Ordered registry and lifecycle owner.
///
/// Services start sequentially in registration order. Shutdown stops running
/// services in reverse registration order. One failure does not stop the
/// remaining services from starting or stopping.
pub struct ServiceManager {
    settings: RuntimeSettings,
    entries: Vec<Entry>,
    permissions: Box<dyn PermissionProvider>,
}

impl ServiceManager {
    pub fn new() -> Self {
        Self::with_settings(RuntimeSettings::default())
    }

    pub fn with_settings(settings: RuntimeSettings) -> Self {
        Self {
            settings,
            entries: Vec::new(),
            permissions: Box::new(UnknownPermissionProvider),
        }
    }

    /// Replace the permission provider. This does not change lifecycle state.
    pub fn set_permission_provider(&mut self, provider: Box<dyn PermissionProvider>) {
        self.permissions = provider;
    }

    pub fn settings(&self) -> &RuntimeSettings {
        &self.settings
    }

    /// Register a service. A repeated service ID is rejected.
    ///
    /// `enabled == false` moves the service from `Registered` to `Disabled`
    /// and leaves it unstarted.
    pub fn register(&mut self, service: Box<dyn Service>, enabled: bool) -> Result<()> {
        let id = service.id();
        if self.contains(id) {
            return Err(runtime_error(
                "register",
                id,
                ServiceState::Registered,
                format!("duplicate service id '{}'", id.as_str()),
            ));
        }
        let version = service.version();
        self.entries.push(Entry {
            service,
            state: ServiceState::Registered,
            enabled,
        });
        log_service(
            id,
            version,
            ServiceState::Registered,
            "service registered",
            None,
        );
        if !enabled {
            self.apply(id, ServiceState::Disabled, "register")?;
            log_service(
                id,
                version,
                ServiceState::Disabled,
                "service disabled",
                None,
            );
        }
        Ok(())
    }

    pub fn contains(&self, id: ServiceId) -> bool {
        self.entries.iter().any(|entry| entry.service.id() == id)
    }

    /// Capture one screenshot from the running screenshot service.
    pub fn capture_screenshot(&mut self) -> Result<CaptureReport> {
        self.running_command(ServiceId::Screenshot, "capture_screenshot", |service| {
            service.capture_screenshot()
        })
    }

    /// Start an explicit video recording. The service must already be running.
    pub fn start_recording(&mut self) -> Result<()> {
        self.running_command(ServiceId::Video, "start_recording", |service| {
            service.start_recording()
        })
    }

    /// Finalize the current video recording.
    pub fn stop_recording(&mut self) -> Result<CaptureReport> {
        self.running_command(ServiceId::Video, "stop_recording", |service| {
            service.stop_recording()
        })
    }

    /// Query the foreground application from the running application tracker.
    pub fn current_application(&mut self) -> Result<ApplicationReport> {
        self.running_command(ServiceId::AppTracker, "current_application", |service| {
            service.current_application()
        })
    }

    /// Read cumulative activity counters from the running activity service.
    pub fn activity_snapshot(&mut self) -> Result<ActivityReport> {
        self.running_command(ServiceId::Activity, "activity_snapshot", |service| {
            service.activity_snapshot()
        })
    }

    /// Query the latest browser URL from the running URL tracker.
    pub fn current_url(&mut self) -> Result<UrlReport> {
        self.running_command(ServiceId::UrlTracker, "current_url", |service| {
            service.current_url()
        })
    }

    /// Read cumulative activity without writing a storage row.
    pub fn observe_activity(&mut self) -> Result<ActivityReport> {
        self.running_command(ServiceId::Activity, "observe_activity", |service| {
            service.observe_activity()
        })
    }

    /// Read the foreground application without writing a storage row.
    pub fn observe_application(&mut self) -> Result<ApplicationReport> {
        self.running_command(ServiceId::AppTracker, "observe_application", |service| {
            service.observe_application()
        })
    }

    /// Read the current URL without writing a storage row.
    pub fn observe_url(&mut self) -> Result<UrlReport> {
        self.running_command(ServiceId::UrlTracker, "observe_url", |service| {
            service.observe_url()
        })
    }

    /// Capture one screenshot file without registering its own interval.
    pub fn capture_interval_screenshot(&mut self) -> Result<CaptureReport> {
        self.running_command(
            ServiceId::Screenshot,
            "capture_interval_screenshot",
            |service| service.capture_interval_screenshot(),
        )
    }

    /// Stop services from writing a second copy of the interval rows.
    pub fn release_interval_flush(&mut self) {
        for entry in &mut self.entries {
            entry.service.release_stop_flush();
        }
    }

    fn running_command<T>(
        &mut self,
        id: ServiceId,
        operation: &'static str,
        call: impl FnOnce(&mut dyn Service) -> Result<T>,
    ) -> Result<T> {
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.service.id() == id)
        else {
            return Err(runtime_error(
                operation,
                id,
                ServiceState::Registered,
                format!("{} is not registered", id.as_str()),
            ));
        };
        if entry.state != ServiceState::Running {
            return Err(runtime_error(
                operation,
                id,
                entry.state,
                format!("{} is not running", id.as_str()),
            ));
        }
        call(entry.service.as_mut())
    }

    pub fn get(&self, id: ServiceId) -> Option<ServiceStatus> {
        self.entries
            .iter()
            .find(|entry| entry.service.id() == id)
            .map(|entry| self.status_of(entry))
    }

    /// Registration order. This is the order used for reports and startup.
    pub fn list(&self) -> Vec<ServiceStatus> {
        self.entries
            .iter()
            .map(|entry| self.status_of(entry))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The registered IDs must equal `expected`, in that order, with names.
    pub fn validate_metadata(&self, expected: &[ServiceId]) -> Result<()> {
        let actual: Vec<ServiceId> = self
            .entries
            .iter()
            .map(|entry| entry.service.id())
            .collect();
        if actual.as_slice() != expected {
            let actual_names: Vec<&str> = actual.iter().map(|id| id.as_str()).collect();
            let expected_names: Vec<&str> = expected.iter().map(|id| id.as_str()).collect();
            return Err(Error::new(
                "service-manager",
                "validate",
                format!("registered services {actual_names:?} do not match {expected_names:?}"),
            ));
        }
        for entry in &self.entries {
            if entry.service.name().trim().is_empty() {
                return Err(runtime_error(
                    "validate",
                    entry.service.id(),
                    entry.state,
                    "display name must not be empty",
                ));
            }
        }
        Ok(())
    }

    pub fn start(&mut self, id: ServiceId) -> Result<()> {
        let state = self.current(id)?;
        if state == ServiceState::Disabled {
            return Err(runtime_error(
                "start",
                id,
                state,
                format!("cannot start service {}: service is disabled", id.as_str()),
            ));
        }
        if state == ServiceState::Running {
            return Err(runtime_error(
                "start",
                id,
                state,
                format!(
                    "cannot start service {}: service is already running",
                    id.as_str()
                ),
            ));
        }
        if !matches!(
            state,
            ServiceState::Registered | ServiceState::Stopped | ServiceState::Restarting
        ) {
            return Err(runtime_error(
                "start",
                id,
                state,
                format!(
                    "cannot start service {}: invalid while {}",
                    id.as_str(),
                    state.as_str()
                ),
            ));
        }
        self.apply(id, ServiceState::Starting, "start")?;
        let version = self.version_of(id)?;
        log_service(
            id,
            version,
            ServiceState::Starting,
            "service starting",
            None,
        );
        self.finish_start(id)
    }

    pub fn stop(&mut self, id: ServiceId) -> Result<()> {
        let state = self.current(id)?;
        if state != ServiceState::Running {
            return Err(runtime_error(
                "stop",
                id,
                state,
                format!(
                    "cannot stop service {}: service is not running",
                    id.as_str()
                ),
            ));
        }
        self.apply(id, ServiceState::Stopping, "stop")?;
        let version = self.version_of(id)?;
        log_service(
            id,
            version,
            ServiceState::Stopping,
            "service stopping",
            None,
        );
        let index = self.position(id)?;
        let started = Instant::now();
        let result = self.entries[index].service.stop();
        let elapsed = started.elapsed();
        match result {
            Ok(()) => {
                self.apply(id, ServiceState::Stopped, "stop")?;
                log_service(id, version, ServiceState::Stopped, "service stopped", None);
                if exceeded_timeout(elapsed, self.settings.shutdown_timeout) {
                    return Err(runtime_error(
                        "stop",
                        id,
                        ServiceState::Stopped,
                        format!(
                            "cannot stop service {}: stop exceeded shutdown timeout of {}ms",
                            id.as_str(),
                            self.settings.shutdown_timeout.as_millis()
                        ),
                    ));
                }
                Ok(())
            }
            Err(error) => {
                self.apply(id, ServiceState::Failed, "stop")?;
                let reason = error.to_string();
                log_service(
                    id,
                    version,
                    ServiceState::Failed,
                    "service failed",
                    Some(&reason),
                );
                Err(runtime_error(
                    "stop",
                    id,
                    ServiceState::Failed,
                    format!("cannot stop service {}: {reason}", id.as_str()),
                )
                .with_source(error))
            }
        }
    }

    /// Stop a running or failed service, then start it again.
    pub fn restart(&mut self, id: ServiceId) -> Result<()> {
        let state = self.current(id)?;
        match state {
            ServiceState::Disabled => {
                return Err(runtime_error(
                    "restart",
                    id,
                    state,
                    format!(
                        "cannot restart service {}: service is disabled",
                        id.as_str()
                    ),
                ));
            }
            ServiceState::Running => {
                self.stop(id)?;
            }
            ServiceState::Stopped | ServiceState::Failed => {}
            other => {
                return Err(runtime_error(
                    "restart",
                    id,
                    other,
                    format!(
                        "cannot restart service {}: invalid while {}",
                        id.as_str(),
                        other.as_str()
                    ),
                ));
            }
        }
        self.apply(id, ServiceState::Restarting, "restart")?;
        let version = self.version_of(id)?;
        log_service(
            id,
            version,
            ServiceState::Restarting,
            "service restarting",
            None,
        );
        self.start(id)
    }

    /// Start enabled services in registration order.
    ///
    /// A failed start leaves that service in `Failed` and the manager continues
    /// with the next service. Disabled services are left unchanged.
    pub fn start_all(&mut self) -> Vec<Error> {
        let ids = self.ids();
        let mut failures = Vec::new();
        for id in ids {
            if self.current(id).ok() == Some(ServiceState::Disabled) {
                continue;
            }
            if let Err(error) = self.start(id) {
                failures.push(error);
            }
        }
        failures
    }

    /// Stop running services in reverse registration order.
    pub fn stop_all(&mut self) -> Vec<Error> {
        let mut ids = self.ids();
        ids.reverse();
        let mut failures = Vec::new();
        for id in ids {
            if self.current(id).ok() != Some(ServiceState::Running) {
                continue;
            }
            if let Err(error) = self.stop(id) {
                failures.push(error);
            }
        }
        failures
    }

    /// Graceful shutdown. Same sequence as [`Self::stop_all`].
    pub fn shutdown(&mut self) -> Vec<Error> {
        self.stop_all()
    }

    /// Let a running service recover its process. In-process services do nothing.
    ///
    /// A crash loop moves that service to `Failed` and leaves the other services running.
    pub fn maintain_running(&mut self) -> Vec<Error> {
        let ids = self.ids();
        let mut failures = Vec::new();
        for id in ids {
            if self.current(id).ok() != Some(ServiceState::Running) {
                continue;
            }
            let index = match self.position(id) {
                Ok(index) => index,
                Err(error) => {
                    failures.push(error);
                    continue;
                }
            };
            if let Err(error) = self.entries[index].service.maintenance() {
                let _ = self.apply(id, ServiceState::Failed, "maintain");
                failures.push(error);
            }
        }
        failures
    }

    pub fn health(&self, id: ServiceId) -> Result<Health> {
        let index = self.position(id)?;
        Ok(self.status_of(&self.entries[index]).health)
    }

    pub fn health_all(&self) -> Vec<(ServiceId, Health)> {
        self.entries
            .iter()
            .map(|entry| (entry.service.id(), self.status_of(entry).health))
            .collect()
    }

    /// Re-check declared capabilities. Disabled services are omitted.
    ///
    /// The result is computed on each call. Permission state is not cached.
    pub fn capability_reports(&self) -> Vec<CapabilityObservation> {
        let mut reports = Vec::new();
        for entry in &self.entries {
            if entry.state != ServiceState::Running {
                continue;
            }
            let health = self.status_of(entry).health.status;
            for requirement in entry.service.capabilities() {
                let permission = self.permissions.check(requirement.capability);
                reports.push(CapabilityObservation {
                    service_id: entry.service.id(),
                    lifecycle: entry.state,
                    health,
                    capability: requirement.capability,
                    required: requirement.required,
                    capability_status: self.permissions.capability_status(requirement.capability),
                    permission: permission.state,
                    reason: permission.reason,
                });
            }
        }
        reports
    }

    pub fn permission_status(
        &self,
        capability: wind_core::Capability,
    ) -> wind_core::PermissionStatus {
        self.permissions.check(capability)
    }

    pub fn permission_capability_status(
        &self,
        capability: wind_core::Capability,
    ) -> wind_core::CapabilityStatus {
        self.permissions.capability_status(capability)
    }

    pub fn can_open_settings(&self, capability: wind_core::Capability) -> bool {
        self.permissions.can_open_settings(capability)
    }

    /// Open settings, then the caller must check again. This does not grant access.
    pub fn open_settings(
        &self,
        capability: wind_core::Capability,
    ) -> std::result::Result<(), wind_core::PermissionError> {
        self.permissions.open_settings(capability)
    }

    fn finish_start(&mut self, id: ServiceId) -> Result<()> {
        let index = self.position(id)?;
        let version = self.entries[index].service.version();
        match self.entries[index].service.start() {
            Ok(()) => {
                self.apply(id, ServiceState::Running, "start")?;
                log_service(id, version, ServiceState::Running, "service started", None);
                Ok(())
            }
            Err(error) => {
                self.apply(id, ServiceState::Failed, "start")?;
                let reason = error.to_string();
                log_service(
                    id,
                    version,
                    ServiceState::Failed,
                    "service failed",
                    Some(&reason),
                );
                Err(runtime_error(
                    "start",
                    id,
                    ServiceState::Failed,
                    format!("cannot start service {}: {reason}", id.as_str()),
                )
                .with_source(error))
            }
        }
    }

    fn apply(&mut self, id: ServiceId, to: ServiceState, operation: &'static str) -> Result<()> {
        let index = self.position(id)?;
        let from = self.entries[index].state;
        match from.transition(to) {
            Ok(next) => {
                self.entries[index].state = next;
                Ok(())
            }
            Err(error) => Err(runtime_error(
                operation,
                id,
                from,
                format!("cannot {operation} service {}: {}", id.as_str(), error),
            )),
        }
    }

    fn current(&self, id: ServiceId) -> Result<ServiceState> {
        let index = self.position(id)?;
        Ok(self.entries[index].state)
    }

    fn version_of(&self, id: ServiceId) -> Result<Version> {
        let index = self.position(id)?;
        Ok(self.entries[index].service.version())
    }

    fn position(&self, id: ServiceId) -> Result<usize> {
        self.entries
            .iter()
            .position(|entry| entry.service.id() == id)
            .ok_or_else(|| {
                Error::new(
                    "service-manager",
                    "lookup",
                    format!("service {} is not registered", id.as_str()),
                )
                .with_service(id.as_str())
            })
    }

    fn ids(&self) -> Vec<ServiceId> {
        self.entries
            .iter()
            .map(|entry| entry.service.id())
            .collect()
    }
}

impl Default for ServiceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceManager {
    fn status_of(&self, entry: &Entry) -> ServiceStatus {
        let (health, readiness) = self.effective_health(entry);
        ServiceStatus {
            descriptor: ServiceDescriptor {
                id: entry.service.id(),
                display_name: entry.service.name().to_string(),
                version: entry.service.version(),
                execution_mode: entry.service.execution_mode(),
                enabled: entry.enabled,
            },
            state: entry.state,
            health,
            readiness,
        }
    }

    fn effective_health(&self, entry: &Entry) -> (Health, Readiness) {
        let mut health = entry.service.health();
        let mut readiness = entry.service.readiness();
        if entry.state != ServiceState::Running {
            return (health, readiness);
        }
        for requirement in entry.service.capabilities() {
            let capability_status = self.permissions.capability_status(requirement.capability);
            let derived = health_from_capability(requirement.required, capability_status);
            let combined = combine_health(health.status, derived);
            if combined != health.status {
                health = Health {
                    status: combined,
                    detail: health_detail(combined),
                };
            }
            readiness = combine_readiness(readiness, capability_status);
        }
        (health, readiness)
    }
}

fn health_detail(status: HealthStatus) -> &'static str {
    match status {
        HealthStatus::PermissionRequired => "required capability is not granted",
        HealthStatus::Unsupported => "required capability is unsupported",
        HealthStatus::Degraded => "capability is unavailable",
        HealthStatus::Unhealthy => "service failed",
        HealthStatus::Healthy => "service is healthy",
        HealthStatus::Unknown => "capability status is unknown",
    }
}

fn combine_readiness(current: Readiness, status: CapabilityStatus) -> Readiness {
    match status {
        CapabilityStatus::PermissionRequired => Readiness::PermissionRequired,
        CapabilityStatus::Available => {
            if current == Readiness::PermissionRequired {
                current
            } else {
                Readiness::Ready
            }
        }
        CapabilityStatus::Unsupported
        | CapabilityStatus::Unavailable
        | CapabilityStatus::Unknown => current,
    }
}

fn runtime_error(
    operation: &'static str,
    id: ServiceId,
    state: ServiceState,
    reason: impl Into<String>,
) -> Error {
    Error::new("service-manager", operation, reason)
        .with_service(id.as_str())
        .with_state(state.as_str())
}

fn log_service(
    id: ServiceId,
    version: Version,
    state: ServiceState,
    message: &str,
    error: Option<&str>,
) {
    let version = version.to_string();
    let state_name = state.as_str();
    emit(&LogFields {
        level: if error.is_some() {
            Level::Error
        } else {
            Level::Info
        },
        component: "service-manager",
        service_id: Some(id.as_str()),
        version: Some(&version),
        platform: None,
        message,
        error,
        state: Some(state_name),
    });
}

fn exceeded_timeout(elapsed: Duration, timeout: Duration) -> bool {
    elapsed > timeout
}

#[cfg(test)]
mod tests {
    use super::{exceeded_timeout, ServiceManager, ServiceStatus};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use wind_core::{
        Capability, CapabilityStatus, Error, ExecutionMode, Health, HealthStatus,
        PermissionProvider, PermissionState, PlaceholderService, Readiness, Result, Service,
        ServiceConfiguration, ServiceId, ServiceState, Version,
    };

    fn placeholder(id: ServiceId) -> PlaceholderService {
        PlaceholderService::new(id, Version::new(0, 1, 0))
    }

    #[derive(Clone)]
    struct CallLog {
        events: Arc<Mutex<Vec<(ServiceId, &'static str)>>>,
    }

    impl CallLog {
        fn new() -> Self {
            Self {
                events: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn push(&self, id: ServiceId, event: &'static str) {
            self.events.lock().expect("call log").push((id, event));
        }

        fn events(&self) -> Vec<(ServiceId, &'static str)> {
            self.events.lock().expect("call log").clone()
        }
    }

    struct MockService {
        id: ServiceId,
        mode: ExecutionMode,
        failures_remaining: u32,
        health: HealthStatus,
        calls: CallLog,
    }

    impl MockService {
        fn new(id: ServiceId, calls: CallLog) -> Self {
            Self {
                id,
                mode: ExecutionMode::InProcess,
                failures_remaining: 0,
                health: HealthStatus::Healthy,
                calls,
            }
        }
    }

    impl Service for MockService {
        fn id(&self) -> ServiceId {
            self.id
        }

        fn name(&self) -> &str {
            self.id.display_name()
        }

        fn version(&self) -> Version {
            Version::new(0, 1, 0)
        }

        fn execution_mode(&self) -> ExecutionMode {
            self.mode
        }

        fn start(&mut self) -> Result<()> {
            self.calls.push(self.id, "start");
            if self.failures_remaining > 0 {
                self.failures_remaining -= 1;
                return Err(
                    Error::new("mock-service", "start", "simulated startup failure")
                        .with_service(self.id.as_str()),
                );
            }
            Ok(())
        }

        fn stop(&mut self) -> Result<()> {
            self.calls.push(self.id, "stop");
            Ok(())
        }

        fn health(&self) -> Health {
            Health {
                status: self.health,
                detail: "mock health",
            }
        }

        fn readiness(&self) -> Readiness {
            Readiness::Unknown
        }

        fn configuration(&self) -> ServiceConfiguration {
            ServiceConfiguration {
                id: self.id,
                version: Version::new(0, 1, 0),
                monitoring_enabled: false,
            }
        }
    }

    fn status(manager: &ServiceManager, id: ServiceId) -> ServiceStatus {
        manager.get(id).expect("registered service")
    }

    #[test]
    fn registry_rejects_duplicates_and_lists_in_registration_order() {
        let mut manager = ServiceManager::new();
        manager
            .register(Box::new(placeholder(ServiceId::Video)), true)
            .unwrap();
        manager
            .register(Box::new(placeholder(ServiceId::Screenshot)), true)
            .unwrap();
        let error = manager
            .register(Box::new(placeholder(ServiceId::Video)), true)
            .unwrap_err();
        let text = error.to_string();
        assert!(text.contains("video"), "{text}");
        assert!(text.contains("duplicate"), "{text}");
        assert!(manager.contains(ServiceId::Screenshot));
        assert!(manager.get(ServiceId::Activity).is_none());
        let ids: Vec<_> = manager
            .list()
            .iter()
            .map(|service| service.descriptor.id)
            .collect();
        assert_eq!(ids, vec![ServiceId::Video, ServiceId::Screenshot]);
        assert_eq!(manager.list()[0].descriptor.version, Version::new(0, 1, 0));
    }

    #[test]
    fn start_and_stop_follow_the_state_machine() {
        let mut manager = ServiceManager::new();
        manager
            .register(Box::new(placeholder(ServiceId::Screenshot)), true)
            .unwrap();
        assert_eq!(
            status(&manager, ServiceId::Screenshot).state,
            ServiceState::Registered
        );
        manager.start(ServiceId::Screenshot).unwrap();
        assert_eq!(
            status(&manager, ServiceId::Screenshot).state,
            ServiceState::Running
        );
        manager.stop(ServiceId::Screenshot).unwrap();
        assert_eq!(
            status(&manager, ServiceId::Screenshot).state,
            ServiceState::Stopped
        );
    }

    #[test]
    fn invalid_transitions_leave_the_current_state() {
        let mut manager = ServiceManager::new();
        manager
            .register(Box::new(placeholder(ServiceId::Activity)), true)
            .unwrap();
        let stop_error = manager.stop(ServiceId::Activity).unwrap_err();
        assert!(
            stop_error.to_string().contains("not running"),
            "{stop_error}"
        );
        assert_eq!(stop_error.service(), Some("activity"));
        assert_eq!(stop_error.state(), Some("Registered"));
        assert_eq!(
            status(&manager, ServiceId::Activity).state,
            ServiceState::Registered
        );

        manager.start(ServiceId::Activity).unwrap();
        let start_error = manager.start(ServiceId::Activity).unwrap_err();
        assert!(
            start_error.to_string().contains("already running"),
            "{start_error}"
        );
        assert_eq!(
            status(&manager, ServiceId::Activity).state,
            ServiceState::Running
        );
    }

    #[test]
    fn disabled_service_does_not_start() {
        let mut manager = ServiceManager::new();
        manager
            .register(Box::new(placeholder(ServiceId::Video)), false)
            .unwrap();
        manager
            .register(Box::new(placeholder(ServiceId::Screenshot)), true)
            .unwrap();
        let failures = manager.start_all();
        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(
            status(&manager, ServiceId::Video).state,
            ServiceState::Disabled
        );
        assert!(!status(&manager, ServiceId::Video).descriptor.enabled);
        assert_eq!(
            status(&manager, ServiceId::Screenshot).state,
            ServiceState::Running
        );
        let error = manager.start(ServiceId::Video).unwrap_err();
        assert!(error.to_string().contains("disabled"), "{error}");
        assert_eq!(
            status(&manager, ServiceId::Video).state,
            ServiceState::Disabled
        );
    }

    #[test]
    fn startup_failure_is_isolated_and_restart_can_recover() {
        let calls = CallLog::new();
        let mut manager = ServiceManager::new();
        manager
            .register(
                Box::new(MockService::new(ServiceId::Screenshot, calls.clone())),
                true,
            )
            .unwrap();
        let mut failing = MockService::new(ServiceId::Activity, calls.clone());
        failing.failures_remaining = 1;
        manager.register(Box::new(failing), true).unwrap();
        manager
            .register(
                Box::new(MockService::new(ServiceId::Video, calls.clone())),
                true,
            )
            .unwrap();

        let failures = manager.start_all();
        assert_eq!(failures.len(), 1);
        let text = failures[0].to_string();
        assert!(text.contains("activity"), "{text}");
        assert!(text.contains("simulated startup failure"), "{text}");
        assert_eq!(failures[0].state(), Some("Failed"));
        assert_eq!(
            status(&manager, ServiceId::Screenshot).state,
            ServiceState::Running
        );
        assert_eq!(
            status(&manager, ServiceId::Activity).state,
            ServiceState::Failed
        );
        assert_eq!(
            status(&manager, ServiceId::Video).state,
            ServiceState::Running
        );

        let second = manager.start(ServiceId::Activity).unwrap_err();
        assert!(second.to_string().contains("invalid"), "{second}");
        assert_eq!(
            status(&manager, ServiceId::Activity).state,
            ServiceState::Failed
        );

        manager.restart(ServiceId::Activity).unwrap();
        assert_eq!(
            status(&manager, ServiceId::Activity).state,
            ServiceState::Running
        );
    }

    #[test]
    fn shutdown_stops_running_services_in_reverse_order() {
        let calls = CallLog::new();
        let mut manager = ServiceManager::new();
        for id in [
            ServiceId::Screenshot,
            ServiceId::Activity,
            ServiceId::AppTracker,
        ] {
            manager
                .register(Box::new(MockService::new(id, calls.clone())), true)
                .unwrap();
        }
        manager
            .register(
                Box::new(MockService::new(ServiceId::Video, calls.clone())),
                false,
            )
            .unwrap();
        assert!(manager.start_all().is_empty());
        let errors = manager.shutdown();
        assert!(errors.is_empty(), "{errors:?}");
        let stops: Vec<_> = calls
            .events()
            .into_iter()
            .filter(|(_, event)| *event == "stop")
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            stops,
            vec![
                ServiceId::AppTracker,
                ServiceId::Activity,
                ServiceId::Screenshot
            ]
        );
        assert_eq!(
            status(&manager, ServiceId::Screenshot).state,
            ServiceState::Stopped
        );
        assert_eq!(
            status(&manager, ServiceId::Video).state,
            ServiceState::Disabled
        );
        assert!(!calls
            .events()
            .iter()
            .any(|(id, event)| { *id == ServiceId::Video && *event == "stop" }));
    }

    #[test]
    fn health_stays_independent_of_lifecycle_state() {
        let mut service = MockService::new(ServiceId::Screenshot, CallLog::new());
        service.health = HealthStatus::Unhealthy;
        let mut manager = ServiceManager::new();
        manager.register(Box::new(service), true).unwrap();
        manager.start(ServiceId::Screenshot).unwrap();
        let current = status(&manager, ServiceId::Screenshot);
        assert_eq!(current.state, ServiceState::Running);
        assert_eq!(current.health.status, HealthStatus::Unhealthy);
        assert_eq!(
            manager.health(ServiceId::Screenshot).unwrap().status,
            HealthStatus::Unhealthy
        );
        assert_eq!(
            manager.health_all()[0],
            (ServiceId::Screenshot, current.health)
        );
    }

    #[test]
    fn validate_metadata_rejects_an_unexpected_set() {
        let mut manager = ServiceManager::new();
        manager
            .register(Box::new(placeholder(ServiceId::Video)), true)
            .unwrap();
        let error = manager.validate_metadata(&ServiceId::ALL).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("video"), "{text}");
        assert!(text.contains("validate"), "{text}");
    }

    #[test]
    fn running_placeholder_reports_healthy_without_monitoring() {
        let service = placeholder(ServiceId::UrlTracker);
        assert!(!service.configuration().monitoring_enabled);
        let mut manager = ServiceManager::new();
        manager.register(Box::new(service), true).unwrap();
        assert_eq!(
            status(&manager, ServiceId::UrlTracker).health.status,
            HealthStatus::Unknown
        );
        manager.start(ServiceId::UrlTracker).unwrap();
        let current = status(&manager, ServiceId::UrlTracker);
        assert_eq!(current.state, ServiceState::Running);
        assert_eq!(current.health.status, HealthStatus::Healthy);
        assert_eq!(current.descriptor.execution_mode, ExecutionMode::InProcess);
        assert!(!current.descriptor.display_name.is_empty());
        manager.stop(ServiceId::UrlTracker).unwrap();
        assert_eq!(
            status(&manager, ServiceId::UrlTracker).health.status,
            HealthStatus::Unknown
        );
    }

    #[test]
    fn timeout_comparison_uses_the_configured_budget() {
        assert!(exceeded_timeout(
            Duration::from_secs(6),
            Duration::from_secs(5)
        ));
        assert!(!exceeded_timeout(
            Duration::from_millis(10),
            Duration::from_secs(5)
        ));
    }

    #[test]
    fn out_of_process_handle_uses_the_same_lifecycle_api() {
        let mut service = MockService::new(ServiceId::Video, CallLog::new());
        service.mode = ExecutionMode::OutOfProcess;
        let mut manager = ServiceManager::new();
        manager.register(Box::new(service), true).unwrap();
        manager.start(ServiceId::Video).unwrap();
        assert_eq!(
            status(&manager, ServiceId::Video).descriptor.execution_mode,
            ExecutionMode::OutOfProcess
        );
        assert_eq!(
            status(&manager, ServiceId::Video).state,
            ServiceState::Running
        );
        assert!(manager.shutdown().is_empty());
        assert_eq!(
            status(&manager, ServiceId::Video).state,
            ServiceState::Stopped
        );
    }

    #[test]
    fn missing_permission_keeps_the_service_running() {
        let mut manager = ServiceManager::new();
        let service = PlaceholderService::new(ServiceId::Screenshot, Version::new(0, 1, 0))
            .requiring(SCREEN_CAPTURE);
        manager.register(Box::new(service), true).unwrap();
        manager.set_permission_provider(Box::new(ScriptedProvider {
            state: Arc::new(Mutex::new(PermissionState::NotDetermined)),
            status: None,
        }));
        assert!(manager.start_all().is_empty());
        let running = status(&manager, ServiceId::Screenshot);
        assert_eq!(running.state, ServiceState::Running);
        assert_eq!(running.health.status, HealthStatus::PermissionRequired);
        assert_eq!(running.readiness, Readiness::PermissionRequired);
        assert_ne!(running.state, ServiceState::Failed);
        let report = &manager.capability_reports()[0];
        assert_eq!(report.capability, Capability::ScreenCapture);
        assert_eq!(report.permission, PermissionState::NotDetermined);
        assert_eq!(
            report.capability_status,
            CapabilityStatus::PermissionRequired
        );
        assert!(report.required);
        let again = manager.permission_status(Capability::ScreenCapture);
        assert_eq!(again.state, PermissionState::NotDetermined);
        manager.set_permission_provider(Box::new(ScriptedProvider {
            state: Arc::new(Mutex::new(PermissionState::Granted)),
            status: None,
        }));
        let upgraded = PlaceholderService::new(ServiceId::Screenshot, Version::new(2, 0, 0))
            .requiring(SCREEN_CAPTURE);
        assert_eq!(upgraded.version().to_string(), "2.0.0");
        assert_eq!(upgraded.id(), ServiceId::Screenshot);
        assert_eq!(
            manager.permission_status(Capability::ScreenCapture).state,
            PermissionState::Granted
        );
        manager.set_permission_provider(Box::new(ScriptedProvider {
            state: Arc::new(Mutex::new(PermissionState::NotDetermined)),
            status: None,
        }));
        assert_eq!(
            manager.permission_status(Capability::ScreenCapture).state,
            PermissionState::NotDetermined
        );

        let provider = ScriptedProvider {
            state: Arc::new(Mutex::new(PermissionState::NotDetermined)),
            status: None,
        };
        provider
            .open_settings(Capability::ScreenCapture)
            .expect("settings open does not grant");
        assert_eq!(
            provider.check(Capability::ScreenCapture).state,
            PermissionState::NotDetermined
        );

        manager.set_permission_provider(Box::new(ScriptedProvider {
            state: Arc::new(Mutex::new(PermissionState::Granted)),
            status: None,
        }));
        let refreshed = status(&manager, ServiceId::Screenshot);
        assert_eq!(refreshed.state, ServiceState::Running);
        assert_eq!(refreshed.health.status, HealthStatus::Healthy);
        assert_eq!(refreshed.readiness, Readiness::Ready);
    }

    #[test]
    fn optional_capability_degrades_without_failing() {
        let mut manager = ServiceManager::new();
        manager
            .register(Box::new(OptionalCapabilityService { started: false }), true)
            .unwrap();
        manager.set_permission_provider(Box::new(ScriptedProvider {
            state: Arc::new(Mutex::new(PermissionState::Denied)),
            status: None,
        }));
        assert!(manager.start(ServiceId::Activity).is_ok());
        let denied = status(&manager, ServiceId::Activity);
        assert_eq!(denied.state, ServiceState::Running);
        assert_eq!(denied.health.status, HealthStatus::Degraded);

        manager.set_permission_provider(Box::new(ScriptedProvider {
            state: Arc::new(Mutex::new(PermissionState::Granted)),
            status: None,
        }));
        let granted = status(&manager, ServiceId::Activity);
        assert_eq!(granted.state, ServiceState::Running);
        assert_eq!(granted.health.status, HealthStatus::Healthy);

        manager.set_permission_provider(Box::new(ScriptedProvider {
            state: Arc::new(Mutex::new(PermissionState::Unknown)),
            status: Some(CapabilityStatus::Unavailable),
        }));
        let unavailable = status(&manager, ServiceId::Activity);
        assert_eq!(unavailable.state, ServiceState::Running);
        assert_eq!(unavailable.health.status, HealthStatus::Degraded);
        assert_ne!(unavailable.state, ServiceState::Failed);
    }

    const SCREEN_CAPTURE: &[wind_core::CapabilityRequirement] =
        &[wind_core::CapabilityRequirement {
            capability: Capability::ScreenCapture,
            required: true,
        }];

    struct ScriptedProvider {
        state: Arc<Mutex<PermissionState>>,
        status: Option<CapabilityStatus>,
    }

    impl PermissionProvider for ScriptedProvider {
        fn check(&self, capability: Capability) -> wind_core::PermissionStatus {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .to_owned();
            wind_core::PermissionStatus::new(capability, state, "scripted permission")
        }

        fn open_settings(
            &self,
            _capability: Capability,
        ) -> std::result::Result<(), wind_core::PermissionError> {
            Ok(())
        }

        fn capability_status(&self, capability: Capability) -> CapabilityStatus {
            self.status.unwrap_or_else(|| {
                wind_core::capability_status_from_permission(self.check(capability).state)
            })
        }
    }

    struct OptionalCapabilityService {
        started: bool,
    }

    impl Service for OptionalCapabilityService {
        fn id(&self) -> ServiceId {
            ServiceId::Activity
        }

        fn name(&self) -> &str {
            "optional"
        }

        fn version(&self) -> Version {
            Version::new(0, 1, 0)
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
                    detail: "optional capability",
                }
            } else {
                Health {
                    status: HealthStatus::Unknown,
                    detail: "stopped",
                }
            }
        }

        fn capabilities(&self) -> &'static [wind_core::CapabilityRequirement] {
            const REQUIREMENTS: &[wind_core::CapabilityRequirement] =
                &[wind_core::CapabilityRequirement {
                    capability: Capability::InputMonitoring,
                    required: false,
                }];
            REQUIREMENTS
        }

        fn configuration(&self) -> ServiceConfiguration {
            ServiceConfiguration {
                id: self.id(),
                version: self.version(),
                monitoring_enabled: false,
            }
        }
    }
}
