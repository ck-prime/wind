//! Out-of-process application tracker.
//!
//! The executable answers foreground queries. Segment storage stays in this
//! process, in the shared store, so the service does not open a second database.

use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use wind_app_tracker::{ActiveApplication, AppError, AppTrackerProvider, AppTrackerService};
use wind_core::{
    emit, ApplicationReport, CapabilityRequirement, CapabilityStatus, Error, ExecutionMode, Health,
    HealthStatus, Level, LogFields, Readiness, Result, Service, ServiceConfiguration, ServiceId,
    Version, SERVICE_PROTOCOL_VERSION,
};
use wind_storage::LocalStore;

use crate::manifest::ServiceInstall;
use crate::process::{LaunchSpec, ServiceProcess};

struct RemoteProvider {
    process: Arc<Mutex<Option<ServiceProcess>>>,
}

impl RemoteProvider {
    fn with_process<T>(
        &self,
        operation: impl FnOnce(&mut ServiceProcess) -> std::result::Result<T, Error>,
    ) -> std::result::Result<T, AppError> {
        let mut guard = self
            .process
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(process) = guard.as_mut() else {
            return Err(AppError::Unavailable {
                reason: "application service process is not running".to_string(),
            });
        };
        operation(process).map_err(|error| AppError::Unavailable {
            reason: error.message().to_string(),
        })
    }
}

impl AppTrackerProvider for RemoteProvider {
    fn current_application(&self) -> std::result::Result<Option<ActiveApplication>, AppError> {
        let message = self.with_process(|process| process.current_application())?;
        let Some(message) = message else {
            return Ok(None);
        };
        if !message.present {
            return Ok(None);
        }
        Ok(Some(ActiveApplication {
            application_id: message.application_id,
            name: if message.name.is_empty() {
                None
            } else {
                Some(message.name)
            },
            executable: if message.executable.is_empty() {
                None
            } else {
                Some(message.executable)
            },
            observed_at: SystemTime::now(),
        }))
    }

    fn capability_status(&self) -> CapabilityStatus {
        self.with_process(|process| process.health())
            .map(|(_, capability)| capability)
            .unwrap_or(CapabilityStatus::Unavailable)
    }
}

/// Application tracker whose platform query runs in `wind-app-tracker`.
pub struct HostedAppTracker {
    version: Version,
    executable: std::path::PathBuf,
    process: Arc<Mutex<Option<ServiceProcess>>>,
    inner: AppTrackerService,
}

impl HostedAppTracker {
    pub fn prepare(install: &ServiceInstall, store: Option<Arc<dyn LocalStore>>) -> Result<Self> {
        crate::process::validate_executable(install.id, &install.executable)?;
        let process = Arc::new(Mutex::new(None));
        let inner = wind_app_tracker::create_with_store(
            Box::new(RemoteProvider {
                process: Arc::clone(&process),
            }),
            store,
        )?;
        Ok(Self {
            version: install.version,
            executable: install.executable.clone(),
            process,
            inner,
        })
    }

    fn log(&self, level: Level, state: &str, message: &str) {
        let version = self.version.to_string();
        emit(&LogFields {
            level,
            component: ServiceId::AppTracker.as_str(),
            service_id: Some(ServiceId::AppTracker.as_str()),
            version: Some(&version),
            platform: None,
            message,
            error: None,
            state: Some(state),
        });
    }
}

impl Service for HostedAppTracker {
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
        ExecutionMode::OutOfProcess
    }

    fn protocol_version(&self) -> u32 {
        SERVICE_PROTOCOL_VERSION
    }

    fn start(&mut self) -> Result<()> {
        self.log(
            Level::Info,
            "Starting",
            &format!(
                "[{}][{}] Starting protocol={SERVICE_PROTOCOL_VERSION} process=Starting",
                ServiceId::AppTracker.as_str(),
                self.version
            ),
        );
        let process = ServiceProcess::launch(LaunchSpec {
            service_id: ServiceId::AppTracker,
            version: self.version,
            executable: self.executable.clone(),
            test_mode: None,
        })?;
        *self
            .process
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(process);
        self.inner.start()?;
        self.log(
            Level::Info,
            "Running",
            &format!(
                "[{}][{}] Running protocol={SERVICE_PROTOCOL_VERSION} process=Running",
                ServiceId::AppTracker.as_str(),
                self.version
            ),
        );
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if let Some(process) = self
            .process
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_mut()
        {
            process.shutdown();
        }
        self.inner.stop()
    }

    fn health(&self) -> Health {
        let running = {
            let mut guard = self
                .process
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard
                .as_mut()
                .map(|process| process.is_running())
                .unwrap_or(false)
        };
        if !running {
            return Health {
                status: HealthStatus::Unhealthy,
                detail: "application service process is not running",
            };
        }
        self.inner.health()
    }

    fn readiness(&self) -> Readiness {
        self.inner.readiness()
    }

    fn capabilities(&self) -> &'static [CapabilityRequirement] {
        self.inner.capabilities()
    }

    fn configuration(&self) -> ServiceConfiguration {
        ServiceConfiguration {
            id: ServiceId::AppTracker,
            version: self.version,
            monitoring_enabled: false,
        }
    }

    fn current_application(&mut self) -> Result<ApplicationReport> {
        Service::current_application(&mut self.inner)
    }

    fn observe_application(&mut self) -> Result<ApplicationReport> {
        self.inner.observe().map_err(|error| {
            Error::new(
                ServiceId::AppTracker.as_str(),
                "observe_application",
                error.to_string(),
            )
            .with_service(ServiceId::AppTracker.as_str())
        })
    }

    fn maintenance(&mut self) -> Result<()> {
        let mut guard = self
            .process
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(process) = guard.as_mut() else {
            return Err(Error::new(
                "supervisor",
                "maintain",
                "application service process is not running",
            )
            .with_service(ServiceId::AppTracker.as_str()));
        };
        let restarts = process.restarts();
        process.recover()?;
        self.log(
            Level::Info,
            "Running",
            &format!(
                "[{}][{}] Running protocol={SERVICE_PROTOCOL_VERSION} process=Running restarts={}",
                ServiceId::AppTracker.as_str(),
                self.version,
                restarts + 1
            ),
        );
        Ok(())
    }
}
