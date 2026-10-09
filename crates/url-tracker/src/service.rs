//! URL tracker service.
//!
//! A URL is read only when the caller asks. Repeating the same browser and URL
//! is not another change. `monitoring_enabled` stays false.

use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use wind_core::{
    emit, Capability, CapabilityRequirement, CapabilityStatus, Error, ExecutionMode, Health,
    HealthStatus, Level, LogFields, Readiness, Result, Service, ServiceConfiguration, ServiceId,
    UrlReport, Version,
};

use wind_storage::{IntervalDraft, LocalStore, UrlSegment};

use crate::contract::{UrlError, UrlObservation, UrlTrackerProvider};

const REQUIREMENTS: &[CapabilityRequirement] = &[CapabilityRequirement {
    capability: Capability::BrowserUrl,
    required: true,
}];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HealthNote {
    Clear,
    Degraded,
    Unhealthy,
}

/// In-process URL tracker.
pub struct UrlTrackerService {
    version: Version,
    provider: Box<dyn UrlTrackerProvider>,
    catalog: Option<Arc<dyn LocalStore>>,
    started: bool,
    last_key: Option<String>,
    open: Option<(String, SystemTime)>,
    note: Mutex<HealthNote>,
}

impl UrlTrackerService {
    pub(crate) fn with_store(
        version: Version,
        provider: Box<dyn UrlTrackerProvider>,
        catalog: Option<Arc<dyn LocalStore>>,
    ) -> Self {
        Self {
            version,
            provider,
            catalog,
            started: false,
            last_key: None,
            open: None,
            note: Mutex::new(HealthNote::Clear),
        }
    }

    /// Read the current URL without writing a segment.
    pub fn observe(&mut self) -> std::result::Result<UrlReport, UrlError> {
        if !self.started {
            return Err(UrlError::Stopped);
        }
        if self.provider.capability_status() == CapabilityStatus::Unsupported {
            return Err(UrlError::Unsupported {
                reason: "url tracking is unsupported".to_string(),
            });
        }
        self.provider.current_url().map(passive_url)
    }

    /// Return the latest accepted URL, and whether it changed since the last query.
    pub fn current_url(&mut self) -> std::result::Result<UrlReport, UrlError> {
        if !self.started {
            return Err(UrlError::Stopped);
        }
        if self.provider.capability_status() == CapabilityStatus::Unsupported {
            return Err(UrlError::Unsupported {
                reason: "url tracking is unsupported".to_string(),
            });
        }
        match self.provider.current_url() {
            Ok(observation) => {
                if let Err(error) = self.note_segment(observation.as_ref()) {
                    self.set_note(HealthNote::Unhealthy);
                    self.log_failure(&error);
                    return Err(error);
                }
                let note = if self.provider.capability_status() == CapabilityStatus::Available {
                    HealthNote::Clear
                } else {
                    HealthNote::Degraded
                };
                self.set_note(note);
                Ok(self.report(observation))
            }
            Err(error @ UrlError::Unsupported { .. }) => {
                self.set_note(HealthNote::Clear);
                self.log_failure(&error);
                Err(error)
            }
            Err(error @ (UrlError::Unavailable { .. } | UrlError::Disconnected { .. })) => {
                let note =
                    if self.provider.capability_status() == CapabilityStatus::PermissionRequired {
                        HealthNote::Clear
                    } else {
                        HealthNote::Degraded
                    };
                self.set_note(note);
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

    fn report(&mut self, observation: Option<UrlObservation>) -> UrlReport {
        let key = observation
            .as_ref()
            .map(|item| format!("{} {}", item.browser.as_str(), item.url));
        let changed = key != self.last_key;
        self.last_key = key;
        if changed {
            self.log_change(observation.as_ref());
        }
        match observation {
            Some(observation) => UrlReport {
                browser: observation.browser.as_str().to_string(),
                url: observation.url,
                present: true,
                changed,
            },
            None => UrlReport {
                browser: String::new(),
                url: String::new(),
                present: false,
                changed,
            },
        }
    }

    fn log_change(&self, observation: Option<&UrlObservation>) {
        let message = match observation {
            Some(observation) => format!("url_observed browser={}", observation.browser.as_str()),
            None => "url_cleared".to_string(),
        };
        let version = self.version.to_string();
        emit(&LogFields {
            level: Level::Info,
            component: ServiceId::UrlTracker.as_str(),
            service_id: Some(ServiceId::UrlTracker.as_str()),
            version: Some(&version),
            platform: None,
            message: &message,
            error: None,
            state: Some("Running"),
        });
    }

    fn log_failure(&self, error: &UrlError) {
        let rendered = error.to_string();
        let version = self.version.to_string();
        emit(&LogFields {
            level: Level::Error,
            component: ServiceId::UrlTracker.as_str(),
            service_id: Some(ServiceId::UrlTracker.as_str()),
            version: Some(&version),
            platform: None,
            message: "url_observation_failed",
            error: Some(&rendered),
            state: Some("Running"),
        });
    }

    fn note_segment(
        &mut self,
        observation: Option<&UrlObservation>,
    ) -> std::result::Result<(), UrlError> {
        if self.catalog.is_none() {
            return Ok(());
        }
        let next = observation.map(|item| item.url.clone());
        let current = self.open.as_ref().map(|(url, _)| url.clone());
        if next == current {
            return Ok(());
        }
        self.close_open(SystemTime::now())?;
        if let Some(url) = next {
            self.open = Some((url, SystemTime::now()));
        }
        Ok(())
    }

    fn close_open(&mut self, ended_at: SystemTime) -> std::result::Result<(), UrlError> {
        let Some(store) = self.catalog.clone() else {
            self.open = None;
            return Ok(());
        };
        let Some((url, started_at)) = self.open.clone() else {
            return Ok(());
        };
        store
            .record_interval(IntervalDraft {
                started_at,
                ended_at,
                activity: None,
                application: None,
                url: Some(UrlSegment {
                    url,
                    started_at,
                    ended_at,
                }),
            })
            .map_err(|_| UrlError::QueryFailed {
                reason: "url usage could not be stored".to_string(),
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

impl Service for UrlTrackerService {
    fn id(&self) -> ServiceId {
        ServiceId::UrlTracker
    }

    fn name(&self) -> &str {
        ServiceId::UrlTracker.display_name()
    }

    fn version(&self) -> Version {
        self.version
    }

    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::InProcess
    }

    fn start(&mut self) -> Result<()> {
        self.provider.start().map_err(|error| {
            Error::new(ServiceId::UrlTracker.as_str(), "start", error.to_string())
                .with_service(ServiceId::UrlTracker.as_str())
        })?;
        self.started = true;
        self.last_key = None;
        self.open = None;
        self.set_note(HealthNote::Clear);
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        let closed = self.close_open(SystemTime::now());
        self.started = false;
        self.last_key = None;
        let stopped = self.provider.stop().map_err(|error| {
            Error::new(ServiceId::UrlTracker.as_str(), "stop", error.to_string())
                .with_service(ServiceId::UrlTracker.as_str())
        });
        closed.map_err(|error| {
            Error::new(ServiceId::UrlTracker.as_str(), "stop", error.to_string())
                .with_service(ServiceId::UrlTracker.as_str())
        })?;
        stopped
    }

    fn health(&self) -> Health {
        if !self.started {
            return Health {
                status: HealthStatus::Unknown,
                detail: "url tracker is not running",
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
            id: ServiceId::UrlTracker,
            version: self.version,
            monitoring_enabled: false,
        }
    }

    fn current_url(&mut self) -> Result<UrlReport> {
        self.current_url().map_err(|error| {
            Error::new(
                ServiceId::UrlTracker.as_str(),
                "current_url",
                error.to_string(),
            )
            .with_service(ServiceId::UrlTracker.as_str())
        })
    }

    fn observe_url(&mut self) -> Result<UrlReport> {
        self.observe().map_err(|error| {
            Error::new(
                ServiceId::UrlTracker.as_str(),
                "observe_url",
                error.to_string(),
            )
            .with_service(ServiceId::UrlTracker.as_str())
        })
    }
}

fn passive_url(observation: Option<UrlObservation>) -> UrlReport {
    match observation {
        Some(observation) => UrlReport {
            browser: observation.browser.as_str().to_string(),
            url: observation.url,
            present: true,
            changed: false,
        },
        None => UrlReport {
            browser: String::new(),
            url: String::new(),
            present: false,
            changed: false,
        },
    }
}

fn health_detail(status: HealthStatus) -> &'static str {
    match status {
        HealthStatus::Healthy => "browser integration is connected",
        HealthStatus::PermissionRequired => "browser integration permission is required",
        HealthStatus::Unsupported => "url tracking is unsupported",
        HealthStatus::Degraded => "browser integration is not connected",
        HealthStatus::Unhealthy => "url observation failed",
        HealthStatus::Unknown => "url tracker is not running",
    }
}

#[cfg(test)]
mod tests {
    use super::UrlTrackerService;
    use crate::{create, BrowserKind, FakeUrlOutcome, FakeUrlTracker, UrlObservation};
    use std::time::{Duration, SystemTime};
    use wind_core::{HealthStatus, Service};

    fn observation(browser: BrowserKind, url: &str) -> UrlObservation {
        UrlObservation {
            browser,
            url: url.to_string(),
            observed_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        }
    }

    fn started(outcome: FakeUrlOutcome) -> (UrlTrackerService, FakeUrlTracker) {
        let fake = FakeUrlTracker::new(outcome);
        let mut service = create(Box::new(fake.clone())).expect("service");
        service.start().expect("start");
        (service, fake)
    }

    #[test]
    fn no_active_browser_returns_none_and_stays_healthy() {
        let (mut service, _) = started(FakeUrlOutcome::Idle);
        let report = service.current_url().expect("query");
        assert!(!report.present);
        assert!(report.url.is_empty());
        assert_eq!(service.health().status, HealthStatus::Healthy);
    }

    #[test]
    fn accessibility_permission_stays_required() {
        let (mut service, _) = started(FakeUrlOutcome::PermissionRequired);
        let error = service.current_url().expect_err("permission");
        let rendered = error.to_string();
        assert!(!rendered.contains("http"));
        assert_eq!(service.health().status, HealthStatus::PermissionRequired);
    }

    #[test]
    fn startup_has_no_url_until_a_browser_sends_one() {
        let (mut service, _) = started(FakeUrlOutcome::None);
        let report = service.current_url().unwrap();
        assert!(!report.present);
        assert!(report.url.is_empty());
        assert_eq!(service.health().status, HealthStatus::Degraded);
        assert!(!service.configuration().monitoring_enabled);
    }

    #[test]
    fn a_repeated_url_is_not_another_change() {
        let (mut service, _) = started(FakeUrlOutcome::Url(observation(
            BrowserKind::Chrome,
            "https://example.com/path?a=1",
        )));
        let first = service.current_url().unwrap();
        let second = service.current_url().unwrap();
        assert!(first.present && first.changed);
        assert_eq!(first.browser, "chrome");
        assert_eq!(first.url, "https://example.com/path?a=1");
        assert!(!second.changed);
        assert_eq!(service.health().status, HealthStatus::Healthy);
    }

    #[test]
    fn a_different_url_is_a_change() {
        let (mut service, fake) = started(FakeUrlOutcome::Url(observation(
            BrowserKind::Edge,
            "https://example.com/",
        )));
        assert!(service.current_url().unwrap().changed);
        fake.set(FakeUrlOutcome::Url(observation(
            BrowserKind::Edge,
            "https://example.org/path",
        )));
        let next = service.current_url().unwrap();
        assert!(next.changed);
        assert_eq!(next.url, "https://example.org/path");
    }

    #[test]
    fn disconnect_clears_the_url_and_stays_running() {
        let (mut service, fake) = started(FakeUrlOutcome::Url(observation(
            BrowserKind::Firefox,
            "https://example.com/",
        )));
        assert!(service.current_url().unwrap().present);
        fake.set(FakeUrlOutcome::Disconnected);
        let cleared = service.current_url().unwrap();
        assert!(!cleared.present);
        assert!(cleared.changed);
        assert_eq!(service.health().status, HealthStatus::Degraded);
    }

    #[test]
    fn unsupported_browser_integration_is_not_a_crash() {
        let (mut service, _) = started(FakeUrlOutcome::Unsupported);
        assert!(matches!(
            service.current_url().unwrap_err(),
            crate::UrlError::Unsupported { .. }
        ));
        assert_eq!(service.health().status, HealthStatus::Unsupported);
    }

    #[test]
    fn query_failure_can_be_retried() {
        let (mut service, fake) = started(FakeUrlOutcome::QueryFailed);
        assert!(service.current_url().is_err());
        assert_eq!(service.health().status, HealthStatus::Unhealthy);
        fake.set(FakeUrlOutcome::Url(observation(
            BrowserKind::Safari,
            "https://example.com/",
        )));
        assert!(service.current_url().unwrap().present);
        assert_eq!(service.health().status, HealthStatus::Healthy);
    }

    #[test]
    fn shutdown_rejects_later_queries() {
        let (mut service, _) = started(FakeUrlOutcome::None);
        service.stop().unwrap();
        assert!(matches!(
            service.current_url().unwrap_err(),
            crate::UrlError::Stopped
        ));
        assert_eq!(service.health().status, HealthStatus::Unknown);
    }

    #[test]
    fn a_finalized_url_keeps_only_the_sanitized_value() {
        fn sanitize(raw: &str) -> Result<String, String> {
            crate::sanitize_http_url(raw).map_err(|_| "url was rejected".to_string())
        }
        let directory = std::env::temp_dir().join(format!("wind-url-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let store = wind_storage::SqliteStore::open(
            &directory,
            wind_storage::StoreOptions {
                max_local_storage_bytes: 1024 * 1024,
                config_version: "0.1.0".to_string(),
                user_id: None,
                sanitize_url: sanitize,
            },
        )
        .expect("open");
        let database = store.database_path().to_path_buf();
        let catalog: std::sync::Arc<dyn wind_storage::LocalStore> = std::sync::Arc::new(store);
        let mut service = crate::create_with_store(
            Box::new(FakeUrlTracker::new(FakeUrlOutcome::Url(observation(
                BrowserKind::Chrome,
                "https://user:password@example.com/path?token=test-secret&a=1#fragment",
            )))),
            Some(catalog),
        )
        .expect("service");
        service.start().expect("start");
        assert!(service.current_url().expect("query").present);
        service.stop().expect("stop");
        drop(service);
        let mut bytes = std::fs::read(&database).unwrap_or_default();
        if let Ok(wal) = std::fs::read(database.with_extension("db-wal")) {
            bytes.extend(wal);
        }
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("https://example.com/path?a=1"));
        assert!(!text.contains("test-secret"));
        assert!(!text.contains("password@"));
        let _ = std::fs::remove_dir_all(directory);
    }
}
