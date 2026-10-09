//! Activity service.
//!
//! The listener starts with the service. A snapshot is taken only when the
//! caller asks. Counters are cumulative. `monitoring_enabled` stays false.
//! When a local store is attached, a non-zero delta is written on snapshot
//! and on stop. The interval engine can take that stop write for itself.
//! Raw input is not written.

use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use wind_core::{
    emit, ActivityReport, Capability, CapabilityRequirement, CapabilityStatus, Error,
    ExecutionMode, Health, HealthStatus, Level, LogFields, Readiness, Result, Service,
    ServiceConfiguration, ServiceId, Version,
};

use wind_storage::{ActivityCounts, IntervalDraft, LocalStore};

use crate::contract::{ActivityError, ActivityProvider, ActivitySnapshot};

const REQUIREMENTS: &[CapabilityRequirement] = &[CapabilityRequirement {
    capability: Capability::InputMonitoring,
    required: true,
}];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HealthNote {
    Clear,
    Unhealthy,
}

/// In-process aggregate activity service.
pub struct ActivityService {
    version: Version,
    provider: Box<dyn ActivityProvider>,
    catalog: Option<Arc<dyn LocalStore>>,
    started: bool,
    window_started: Option<SystemTime>,
    baseline: Option<ActivitySnapshot>,
    flush_on_stop: bool,
    note: Mutex<HealthNote>,
}

impl ActivityService {
    pub(crate) fn with_store(
        version: Version,
        provider: Box<dyn ActivityProvider>,
        catalog: Option<Arc<dyn LocalStore>>,
    ) -> Self {
        Self {
            version,
            provider,
            catalog,
            started: false,
            window_started: None,
            baseline: None,
            flush_on_stop: true,
            note: Mutex::new(HealthNote::Clear),
        }
    }

    /// Try the listener again when access was missing at startup and is on now.
    fn current_capability(&mut self) -> CapabilityStatus {
        if self.provider.capability_status() == CapabilityStatus::PermissionRequired {
            let _ = self.provider.start();
        }
        self.provider.capability_status()
    }

    /// Read the cumulative counters without writing a row or moving the baseline.
    pub fn observe(&mut self) -> std::result::Result<ActivitySnapshot, ActivityError> {
        if !self.started {
            return Err(ActivityError::Stopped);
        }
        match self.current_capability() {
            CapabilityStatus::Available => {}
            CapabilityStatus::PermissionRequired => {
                return Err(ActivityError::PermissionRequired {
                    reason: "input monitoring permission is required".to_string(),
                });
            }
            CapabilityStatus::Unsupported => {
                return Err(ActivityError::Unsupported {
                    reason: "input monitoring is unsupported".to_string(),
                });
            }
            CapabilityStatus::Unavailable | CapabilityStatus::Unknown => {
                return Err(ActivityError::Unavailable {
                    reason: "input monitoring is unavailable".to_string(),
                });
            }
        }
        self.provider.snapshot()
    }

    /// Read the cumulative counters. This does not reset them.
    pub fn snapshot(&mut self) -> std::result::Result<ActivitySnapshot, ActivityError> {
        if !self.started {
            return Err(ActivityError::Stopped);
        }
        match self.current_capability() {
            CapabilityStatus::Available => {}
            CapabilityStatus::PermissionRequired => {
                return Err(ActivityError::PermissionRequired {
                    reason: "input monitoring permission is required".to_string(),
                });
            }
            CapabilityStatus::Unsupported => {
                return Err(ActivityError::Unsupported {
                    reason: "input monitoring is unsupported".to_string(),
                });
            }
            CapabilityStatus::Unavailable | CapabilityStatus::Unknown => {
                return Err(ActivityError::Unavailable {
                    reason: "input monitoring is unavailable".to_string(),
                });
            }
        }
        match self.provider.snapshot() {
            Ok(snapshot) => match self.persist_from(&snapshot) {
                Ok(()) => {
                    self.set_note(HealthNote::Clear);
                    Ok(snapshot)
                }
                Err(error) => {
                    self.set_note(HealthNote::Unhealthy);
                    self.log_failure("activity_store_failed", &error);
                    Err(error)
                }
            },
            Err(error @ ActivityError::Unavailable { .. }) => {
                self.log_failure("activity_provider_unavailable", &error);
                Err(error)
            }
            Err(error) => {
                self.set_note(HealthNote::Unhealthy);
                self.log_failure("activity_provider_failed", &error);
                Err(error)
            }
        }
    }

    fn log_lifecycle(&self, message: &str) {
        let version = self.version.to_string();
        emit(&LogFields {
            level: Level::Info,
            component: ServiceId::Activity.as_str(),
            service_id: Some(ServiceId::Activity.as_str()),
            version: Some(&version),
            platform: None,
            message,
            error: None,
            state: Some("Running"),
        });
    }

    fn log_failure(&self, message: &str, error: &ActivityError) {
        let rendered = error.to_string();
        let version = self.version.to_string();
        emit(&LogFields {
            level: Level::Error,
            component: ServiceId::Activity.as_str(),
            service_id: Some(ServiceId::Activity.as_str()),
            version: Some(&version),
            platform: None,
            message,
            error: Some(&rendered),
            state: Some("Running"),
        });
    }

    fn persist_from(
        &mut self,
        snapshot: &ActivitySnapshot,
    ) -> std::result::Result<(), ActivityError> {
        let Some(store) = &self.catalog else {
            return Ok(());
        };
        let Some(started) = self.window_started else {
            return Ok(());
        };
        let baseline = self.baseline.unwrap_or(ActivitySnapshot {
            observed_at: started,
            keyboard_events: 0,
            mouse_moves: 0,
            mouse_clicks: 0,
            mouse_scrolls: 0,
        });
        let delta = snapshot.delta_since(&baseline);
        if delta.keyboard_events == 0
            && delta.mouse_moves == 0
            && delta.mouse_clicks == 0
            && delta.mouse_scrolls == 0
        {
            return Ok(());
        }
        let ended = SystemTime::now();
        store
            .record_interval(IntervalDraft {
                started_at: started,
                ended_at: ended,
                activity: Some(ActivityCounts {
                    keyboard_events: delta.keyboard_events,
                    mouse_moves: delta.mouse_moves,
                    mouse_clicks: delta.mouse_clicks,
                    mouse_scrolls: delta.mouse_scrolls,
                }),
                application: None,
                url: None,
            })
            .map_err(|_| ActivityError::Unavailable {
                reason: "activity could not be stored".to_string(),
            })?;
        self.baseline = Some(*snapshot);
        self.window_started = Some(ended);
        Ok(())
    }

    fn set_note(&self, note: HealthNote) {
        *self
            .note
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = note;
    }
}

impl Service for ActivityService {
    fn id(&self) -> ServiceId {
        ServiceId::Activity
    }

    fn name(&self) -> &str {
        ServiceId::Activity.display_name()
    }

    fn version(&self) -> Version {
        self.version
    }

    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::InProcess
    }

    fn start(&mut self) -> Result<()> {
        self.provider.start().map_err(|error| {
            self.log_failure("activity_provider_failed", &error);
            Error::new(ServiceId::Activity.as_str(), "start", error.to_string())
                .with_service(ServiceId::Activity.as_str())
        })?;
        self.started = true;
        self.window_started = Some(SystemTime::now());
        self.baseline = self.provider.snapshot().ok();
        self.set_note(HealthNote::Clear);
        match self.provider.capability_status() {
            CapabilityStatus::Available => self.log_lifecycle("activity_listener_started"),
            CapabilityStatus::PermissionRequired => {
                self.log_lifecycle("activity_permission_required")
            }
            CapabilityStatus::Unsupported
            | CapabilityStatus::Unavailable
            | CapabilityStatus::Unknown => self.log_lifecycle("activity_provider_unavailable"),
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        let stored = if self.started && self.flush_on_stop {
            match self.provider.snapshot() {
                Ok(snapshot) => self.persist_from(&snapshot),
                Err(_) => Ok(()),
            }
        } else {
            Ok(())
        };
        let stopped = self.provider.stop().map_err(|error| {
            Error::new(ServiceId::Activity.as_str(), "stop", error.to_string())
                .with_service(ServiceId::Activity.as_str())
        });
        self.started = false;
        self.window_started = None;
        if stopped.is_ok() {
            self.log_lifecycle("activity_listener_stopped");
        }
        if let Err(error) = stored {
            return Err(
                Error::new(ServiceId::Activity.as_str(), "stop", error.to_string())
                    .with_service(ServiceId::Activity.as_str()),
            );
        }
        stopped
    }

    fn health(&self) -> Health {
        if !self.started {
            return Health {
                status: HealthStatus::Unknown,
                detail: "activity tracker is not running",
            };
        }
        let note = *self
            .note
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let status = match note {
            HealthNote::Unhealthy => HealthStatus::Unhealthy,
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
            id: ServiceId::Activity,
            version: self.version,
            monitoring_enabled: false,
        }
    }

    fn activity_snapshot(&mut self) -> Result<ActivityReport> {
        self.snapshot().map(report_from).map_err(|error| {
            Error::new(
                ServiceId::Activity.as_str(),
                "activity_snapshot",
                error.to_string(),
            )
            .with_service(ServiceId::Activity.as_str())
        })
    }

    fn observe_activity(&mut self) -> Result<ActivityReport> {
        self.observe().map(report_from).map_err(|error| {
            Error::new(
                ServiceId::Activity.as_str(),
                "observe_activity",
                error.to_string(),
            )
            .with_service(ServiceId::Activity.as_str())
        })
    }

    fn release_stop_flush(&mut self) {
        self.flush_on_stop = false;
    }
}

fn report_from(snapshot: ActivitySnapshot) -> ActivityReport {
    ActivityReport {
        observed_at: snapshot.observed_at,
        keyboard_events: snapshot.keyboard_events,
        mouse_moves: snapshot.mouse_moves,
        mouse_clicks: snapshot.mouse_clicks,
        mouse_scrolls: snapshot.mouse_scrolls,
    }
}

fn health_detail(status: HealthStatus) -> &'static str {
    match status {
        HealthStatus::Healthy => "aggregate input counters are available",
        HealthStatus::PermissionRequired => "input monitoring permission is required",
        HealthStatus::Unsupported => "input monitoring is unsupported",
        HealthStatus::Degraded => "input monitoring is unavailable",
        HealthStatus::Unhealthy => "activity provider failed",
        HealthStatus::Unknown => "activity tracker is not running",
    }
}

#[cfg(test)]
mod tests {
    use super::ActivityService;
    use crate::contract::{ActivityCounters, FakeActivityOutcome, FakeActivityProvider};
    use crate::create;
    use wind_core::{HealthStatus, Service};

    fn service(outcome: FakeActivityOutcome) -> (ActivityService, ActivityCounters) {
        let counters = ActivityCounters::new();
        let provider = FakeActivityProvider::with_counters(outcome, counters.clone());
        let service = create(Box::new(provider)).unwrap();
        (service, counters)
    }

    #[test]
    fn startup_does_not_require_a_snapshot() {
        let (mut service, _) = service(FakeActivityOutcome::Available);
        service.start().unwrap();
        assert_eq!(service.health().status, HealthStatus::Healthy);
        assert!(!service.configuration().monitoring_enabled);
        let snapshot = service.snapshot().unwrap();
        assert_eq!(snapshot.keyboard_events, 0);
    }

    #[test]
    fn repeated_events_stay_cumulative() {
        let (mut service, counters) = service(FakeActivityOutcome::Available);
        service.start().unwrap();
        let first = service.snapshot().unwrap();
        counters.record_keyboard_event();
        counters.record_mouse_move();
        counters.record_mouse_click();
        counters.record_mouse_scroll();
        let second = service.snapshot().unwrap();
        assert!(second.keyboard_events > first.keyboard_events);
        assert!(second.mouse_moves > first.mouse_moves);
        assert!(second.mouse_clicks > first.mouse_clicks);
        assert!(second.mouse_scrolls > first.mouse_scrolls);
        let third = service.snapshot().unwrap();
        assert_eq!(third.keyboard_events, second.keyboard_events);
        assert_eq!(third.delta_since(&second).keyboard_events, 0);
    }

    #[test]
    fn permission_required_stays_running() {
        let (mut service, _) = service(FakeActivityOutcome::PermissionRequired);
        service.start().unwrap();
        assert_eq!(service.health().status, HealthStatus::PermissionRequired);
        assert!(service.snapshot().is_err());
        assert_eq!(service.health().status, HealthStatus::PermissionRequired);
    }

    #[test]
    fn unsupported_is_not_a_crash() {
        let (mut service, _) = service(FakeActivityOutcome::Unsupported);
        service.start().unwrap();
        assert_eq!(service.health().status, HealthStatus::Unsupported);
    }

    #[test]
    fn unavailable_degrades() {
        let (mut service, _) = service(FakeActivityOutcome::Unavailable);
        service.start().unwrap();
        assert_eq!(service.health().status, HealthStatus::Degraded);
    }

    #[test]
    fn provider_failure_is_unhealthy_and_still_started() {
        let (mut service, _) = service(FakeActivityOutcome::Failure);
        service.start().unwrap();
        assert!(service.snapshot().is_err());
        assert_eq!(service.health().status, HealthStatus::Unhealthy);
    }

    #[test]
    fn stopped_service_rejects_a_snapshot() {
        let (mut service, _) = service(FakeActivityOutcome::Available);
        service.start().unwrap();
        service.stop().unwrap();
        assert!(service.snapshot().is_err());
        assert_eq!(service.health().status, HealthStatus::Unknown);
        service.stop().unwrap();
        service.start().unwrap();
        assert_eq!(service.health().status, HealthStatus::Healthy);
    }
}
