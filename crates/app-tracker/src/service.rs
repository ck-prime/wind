//! Application tracker service.
//!
//! Queries run when the caller asks. The same application id does not produce
//! another change. `monitoring_enabled` stays false.

use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use wind_core::{
    emit, ApplicationReport, Capability, CapabilityRequirement, CapabilityStatus, Error,
    ExecutionMode, Health, HealthStatus, Level, LogFields, Readiness, Result, Service,
    ServiceConfiguration, ServiceId, Version,
};

use wind_storage::{ApplicationSegment, IntervalDraft, LocalStore};

use crate::contract::{ActiveApplication, AppError, AppTrackerProvider};

const REQUIREMENTS: &[CapabilityRequirement] = &[CapabilityRequirement {
    capability: Capability::ActiveApplication,
    required: true,
}];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HealthNote {
    Clear,
    Degraded,
    Unhealthy,
}

/// In-process application tracker.
pub struct AppTrackerService {
    version: Version,
    provider: Box<dyn AppTrackerProvider>,
    catalog: Option<Arc<dyn LocalStore>>,
    started: bool,
    last_id: Option<String>,
    open: Option<(String, SystemTime)>,
    note: Mutex<HealthNote>,
}

impl AppTrackerService {
    pub(crate) fn with_store(
        version: Version,
        provider: Box<dyn AppTrackerProvider>,
        catalog: Option<Arc<dyn LocalStore>>,
    ) -> Self {
        Self {
            version,
            provider,
            catalog,
            started: false,
            last_id: None,
            open: None,
            note: Mutex::new(HealthNote::Clear),
        }
    }

    /// Read the foreground application without writing a segment.
    pub fn observe(&mut self) -> std::result::Result<ApplicationReport, AppError> {
        if !self.started {
            return Err(AppError::Stopped);
        }
        match self.provider.capability_status() {
            CapabilityStatus::Available => {}
            CapabilityStatus::PermissionRequired => {
                return Err(AppError::Unavailable {
                    reason: "application tracking permission is required".to_string(),
                });
            }
            CapabilityStatus::Unsupported => {
                return Err(AppError::Unsupported {
                    reason: "application tracking is unsupported".to_string(),
                });
            }
            CapabilityStatus::Unavailable | CapabilityStatus::Unknown => {
                return Err(AppError::Unavailable {
                    reason: "application tracking is unavailable".to_string(),
                });
            }
        }
        self.provider.current_application().map(passive_application)
    }

    /// Query the foreground application and report whether its identity changed.
    pub fn current_application(&mut self) -> std::result::Result<ApplicationReport, AppError> {
        if !self.started {
            return Err(AppError::Stopped);
        }
        match self.provider.capability_status() {
            CapabilityStatus::Available => {}
            CapabilityStatus::PermissionRequired => {
                return Err(AppError::Unavailable {
                    reason: "application tracking permission is required".to_string(),
                });
            }
            CapabilityStatus::Unsupported => {
                return Err(AppError::Unsupported {
                    reason: "application tracking is unsupported".to_string(),
                });
            }
            CapabilityStatus::Unavailable | CapabilityStatus::Unknown => {
                return Err(AppError::Unavailable {
                    reason: "application tracking is unavailable".to_string(),
                });
            }
        }
        match self.provider.current_application() {
            Ok(application) => {
                if let Err(error) = self.note_segment(application.as_ref()) {
                    self.set_note(HealthNote::Unhealthy);
                    self.log_failure(&error);
                    return Err(error);
                }
                self.set_note(HealthNote::Clear);
                Ok(self.report(application))
            }
            Err(error @ AppError::Unavailable { .. }) => {
                self.set_note(HealthNote::Degraded);
                self.log_failure(&error);
                Err(error)
            }
            Err(error) => {
                self.set_note(HealthNote::Unhealthy);
                self.log_failure(&error);
                Err(error)
            }
        }
    }

    fn report(&mut self, application: Option<ActiveApplication>) -> ApplicationReport {
        let current_id = application.as_ref().map(|item| item.application_id.clone());
        let changed = current_id != self.last_id;
        self.last_id = current_id;
        if changed {
            self.log_change(application.as_ref());
        }
        match application {
            Some(application) => ApplicationReport {
                application_id: application.application_id,
                name: application.name.unwrap_or_default(),
                executable: application.executable.unwrap_or_default(),
                present: true,
                changed,
            },
            None => ApplicationReport {
                application_id: String::new(),
                name: String::new(),
                executable: String::new(),
                present: false,
                changed,
            },
        }
    }

    fn log_change(&self, application: Option<&ActiveApplication>) {
        let message = match application {
            Some(application) => format!("application_changed {}", safe_label(application)),
            None => "application_cleared".to_string(),
        };
        let version = self.version.to_string();
        emit(&LogFields {
            level: Level::Info,
            component: ServiceId::AppTracker.as_str(),
            service_id: Some(ServiceId::AppTracker.as_str()),
            version: Some(&version),
            platform: None,
            message: &message,
            error: None,
            state: Some("Running"),
        });
    }

    fn log_failure(&self, error: &AppError) {
        let message = "application_query_failed";
        let rendered = error.to_string();
        let version = self.version.to_string();
        emit(&LogFields {
            level: Level::Error,
            component: ServiceId::AppTracker.as_str(),
            service_id: Some(ServiceId::AppTracker.as_str()),
            version: Some(&version),
            platform: None,
            message,
            error: Some(&rendered),
            state: Some("Running"),
        });
    }

    fn note_segment(
        &mut self,
        application: Option<&ActiveApplication>,
    ) -> std::result::Result<(), AppError> {
        if self.catalog.is_none() {
            return Ok(());
        }
        let next = application.map(|item| item.application_id.clone());
        if next.as_deref() == self.open.as_ref().map(|(id, _)| id.as_str()) {
            return Ok(());
        }
        self.close_open(SystemTime::now())?;
        if let Some(application_id) = next {
            self.open = Some((application_id, SystemTime::now()));
        }
        Ok(())
    }

    fn close_open(&mut self, ended_at: SystemTime) -> std::result::Result<(), AppError> {
        let Some(store) = self.catalog.clone() else {
            self.open = None;
            return Ok(());
        };
        let Some((application_id, started_at)) = self.open.clone() else {
            return Ok(());
        };
        store
            .record_interval(IntervalDraft {
                started_at,
                ended_at,
                activity: None,
                application: Some(ApplicationSegment {
                    application_id,
                    started_at,
                    ended_at,
                }),
                url: None,
            })
            .map_err(|_| AppError::QueryFailed {
                reason: "application usage could not be stored".to_string(),
            })?;
        self.open = None;
        Ok(())
    }

    fn set_note(&self, note: HealthNote) {
        *self
            .note
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = note;
    }
}

fn passive_application(application: Option<ActiveApplication>) -> ApplicationReport {
    match application {
        Some(application) => ApplicationReport {
            application_id: application.application_id,
            name: application.name.unwrap_or_default(),
            executable: application.executable.unwrap_or_default(),
            present: true,
            changed: false,
        },
        None => ApplicationReport {
            application_id: String::new(),
            name: String::new(),
            executable: String::new(),
            present: false,
            changed: false,
        },
    }
}

fn safe_label(application: &ActiveApplication) -> String {
    if let Some(name) = application.name.as_deref().filter(|name| !name.is_empty()) {
        return format!("name={name}");
    }
    if !application.application_id.contains('\\') && !application.application_id.contains('/') {
        return format!("application_id={}", application.application_id);
    }
    "name=-".to_string()
}

impl Service for AppTrackerService {
    fn id(&self) -> ServiceId {
        ServiceId::AppTracker
    }

    fn name(&self) -> &str {
        ServiceId::AppTracker.display_name()
    }

    fn version(&self) -> Version {
        self.version
    }

    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::InProcess
    }

    fn start(&mut self) -> Result<()> {
        self.started = true;
        self.last_id = None;
        self.open = None;
        self.set_note(HealthNote::Clear);
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        let closed = self.close_open(SystemTime::now());
        self.started = false;
        self.last_id = None;
        closed.map_err(|error| {
            Error::new(ServiceId::AppTracker.as_str(), "stop", error.to_string())
                .with_service(ServiceId::AppTracker.as_str())
        })
    }

    fn health(&self) -> Health {
        if !self.started {
            return Health {
                status: HealthStatus::Unknown,
                detail: "application tracker is not running",
            };
        }
        let note = *self
            .note
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let status = match note {
            HealthNote::Unhealthy => HealthStatus::Unhealthy,
            HealthNote::Degraded => HealthStatus::Degraded,
            HealthNote::Clear => match self.provider.capability_status() {
                CapabilityStatus::Available => HealthStatus::Healthy,
                CapabilityStatus::PermissionRequired => HealthStatus::PermissionRequired,
                CapabilityStatus::Unsupported => HealthStatus::Unsupported,
                CapabilityStatus::Unavailable => HealthStatus::Degraded,
                CapabilityStatus::Unknown => HealthStatus::Unknown,
            },
        };
        Health {
            status,
            detail: health_detail(status),
        }
    }

    fn readiness(&self) -> Readiness {
        match self.health().status {
            HealthStatus::Healthy => Readiness::Ready,
            HealthStatus::PermissionRequired => Readiness::PermissionRequired,
            _ => Readiness::Unknown,
        }
    }

    fn capabilities(&self) -> &'static [CapabilityRequirement] {
        REQUIREMENTS
    }

    fn configuration(&self) -> ServiceConfiguration {
        ServiceConfiguration {
            id: ServiceId::AppTracker,
            version: self.version,
            monitoring_enabled: false,
        }
    }

    fn current_application(&mut self) -> Result<ApplicationReport> {
        self.current_application().map_err(|error| {
            Error::new(
                ServiceId::AppTracker.as_str(),
                "current_application",
                error.to_string(),
            )
            .with_service(ServiceId::AppTracker.as_str())
        })
    }

    fn observe_application(&mut self) -> Result<ApplicationReport> {
        self.observe().map_err(|error| {
            Error::new(
                ServiceId::AppTracker.as_str(),
                "observe_application",
                error.to_string(),
            )
            .with_service(ServiceId::AppTracker.as_str())
        })
    }
}

fn health_detail(status: HealthStatus) -> &'static str {
    match status {
        HealthStatus::Healthy => "foreground application query is available",
        HealthStatus::PermissionRequired => "application tracking permission is required",
        HealthStatus::Unsupported => "application tracking is unsupported",
        HealthStatus::Degraded => "foreground application is unavailable",
        HealthStatus::Unhealthy => "foreground application query failed",
        HealthStatus::Unknown => "application tracker is not running",
    }
}

#[cfg(test)]
mod tests {
    use super::AppTrackerService;
    use crate::{create, ActiveApplication, AppError, FakeAppOutcome, FakeAppTracker};
    use std::time::{Duration, SystemTime};
    use wind_core::{HealthStatus, Service};

    fn application(id: &str, name: &str) -> ActiveApplication {
        ActiveApplication {
            application_id: id.to_string(),
            name: Some(name.to_string()),
            executable: Some(format!("/usr/bin/{name}")),
            observed_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        }
    }

    fn started(outcome: FakeAppOutcome) -> (AppTrackerService, FakeAppTracker) {
        let fake = FakeAppTracker::new(outcome);
        let mut service = create(Box::new(fake.clone())).expect("service");
        service.start().expect("start");
        (service, fake)
    }

    #[test]
    fn startup_does_not_require_an_observation() {
        let mut service = create(Box::new(FakeAppTracker::new(FakeAppOutcome::None))).unwrap();
        assert_eq!(service.health().status, HealthStatus::Unknown);
        service.start().unwrap();
        assert_eq!(service.health().status, HealthStatus::Healthy);
        assert!(!service.configuration().monitoring_enabled);
    }

    #[test]
    fn repeated_identity_is_not_another_change() {
        let (mut service, _) = started(FakeAppOutcome::Application(application(
            "com.example.Editor",
            "Editor",
        )));
        let first = service.current_application().unwrap();
        let second = service.current_application().unwrap();
        assert!(first.present && first.changed);
        assert_eq!(first.application_id, "com.example.Editor");
        assert_eq!(first.name, "Editor");
        assert!(!second.changed);
        assert_eq!(second.application_id, first.application_id);
        let observed = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert!(observed.elapsed().is_ok());
    }

    #[test]
    fn a_different_application_is_a_change() {
        let (mut service, fake) = started(FakeAppOutcome::Application(application("a", "A")));
        assert!(service.current_application().unwrap().changed);
        fake.set(FakeAppOutcome::Application(application("b", "B")));
        let next = service.current_application().unwrap();
        assert!(next.changed);
        assert_eq!(next.application_id, "b");
        fake.set(FakeAppOutcome::None);
        let cleared = service.current_application().unwrap();
        assert!(cleared.changed);
        assert!(!cleared.present);
    }

    #[test]
    fn no_foreground_application_is_empty() {
        let (mut service, _) = started(FakeAppOutcome::None);
        let report = service.current_application().unwrap();
        assert!(!report.present);
        assert!(!report.changed);
        assert!(report.application_id.is_empty());
    }

    #[test]
    fn unsupported_stays_running() {
        let (mut service, _) = started(FakeAppOutcome::Unsupported);
        let error = service.current_application().unwrap_err();
        assert!(matches!(error, AppError::Unsupported { .. }));
        assert_eq!(service.health().status, HealthStatus::Unsupported);
        service.stop().unwrap();
        assert_eq!(service.health().status, HealthStatus::Unknown);
    }

    #[test]
    fn unavailable_degrades_without_stopping() {
        let (mut service, _) = started(FakeAppOutcome::Unavailable);
        assert!(matches!(
            service.current_application().unwrap_err(),
            AppError::Unavailable { .. }
        ));
        assert_eq!(service.health().status, HealthStatus::Degraded);
        assert_eq!(
            service.start().ok().map(|_| service.health().status),
            Some(HealthStatus::Degraded)
        );
    }

    #[test]
    fn query_failure_is_unhealthy_and_can_be_retried() {
        let (mut service, fake) = started(FakeAppOutcome::QueryFailed);
        assert!(matches!(
            service.current_application().unwrap_err(),
            AppError::QueryFailed { .. }
        ));
        assert_eq!(service.health().status, HealthStatus::Unhealthy);
        fake.set(FakeAppOutcome::Application(application("a", "A")));
        assert!(service.current_application().unwrap().present);
        assert_eq!(service.health().status, HealthStatus::Healthy);
    }

    #[test]
    fn stopped_service_rejects_queries() {
        let (mut service, _) = started(FakeAppOutcome::None);
        service.stop().unwrap();
        assert!(matches!(
            service.current_application().unwrap_err(),
            AppError::Stopped
        ));
    }
}
