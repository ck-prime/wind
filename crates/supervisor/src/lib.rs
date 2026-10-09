//! Supervisor composition root.
//!
//! Loads configuration, builds one runtime context, and keeps the process
//! resident until shutdown. While it is resident it closes 10-second tracking
//! intervals. The screenshot service can capture one display when asked. Video
//! is recorded only when start_recording is called. Application and URL queries
//! are explicit. Activity counts input volume when the service is running and
//! reports it when a snapshot is requested. Finalized tracking records and
//! artifact metadata are written to the local store. This process
//! does not store keystrokes or pointer coordinates, request permissions,
//! authenticate, connect to a network, update components, or install an
//! operating-system service.

mod hosted;
mod interval;
mod manifest;
mod process;
mod runtime_state;
mod shutdown;
#[cfg(debug_assertions)]
mod test_mode;
mod updates;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use wind_config::ProductConfig;
use wind_core::{
    emit, Capability, CapabilityStatus, CaptureReport, Error, HealthStatus, Level, LogFields,
    PermissionState, Service, ServiceId, ServiceState, Version, VersionError, SUPERVISOR_COMPONENT,
};

use wind_platform::{
    host_activity_provider, host_app_tracker, host_screenshot_provider, host_url_tracker,
    host_video_provider, HostPermissionProvider, PlatformInfo, PlatformRuntime, RuntimePaths,
    SessionContext,
};
use wind_service_manager::{CapabilityObservation, RuntimeSettings, ServiceManager, ServiceStatus};
use wind_storage::{
    ActivityCounts, ApplicationSegment, ArtifactKind, ArtifactRequest, IntervalDraft, LocalStore,
    SqliteStore, StoreOptions, UrlSegment, DEFAULT_MAX_LOCAL_STORAGE_BYTES,
};

pub use process::{LaunchSpec, ServiceProcess};
pub use runtime_state::{InvalidRuntimeTransition, RuntimeState};
pub use shutdown::ShutdownCoordinator;
#[cfg(debug_assertions)]
pub use test_mode::run_service_test_mode;
pub use updates::{ClosedVerifier, FixtureVerifier, PackageMetadata, UpdateManager};

/// Facts resolved once during startup. Services read these instead of the OS.
#[derive(Debug, Clone)]
pub struct RuntimeContext {
    pub product: ProductConfig,
    pub platform: PlatformInfo,
    pub paths: RuntimePaths,
    pub session: SessionContext,
    pub health_check_interval: Duration,
    pub shutdown_timeout: Duration,
}

/// Foreground supervisor session.
pub struct Session {
    context: RuntimeContext,
    supervisor_version: Version,
    manager: ServiceManager,
    store: Arc<dyn LocalStore>,
    start_failures: Vec<Error>,
    runtime_state: RuntimeState,
}

/// Counts from one health pass. Lifecycle state is not part of this result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthPoll {
    pub healthy: usize,
    pub degraded: usize,
    pub unhealthy: usize,
    pub unknown: usize,
    pub permission_required: usize,
    pub unsupported: usize,
}

/// Runtime snapshot taken after startup.
#[derive(Debug, Clone)]
pub struct RuntimeReport {
    pub display_name: String,
    pub product_version: Version,
    pub platform_name: String,
    pub architecture: String,
    pub user: String,
    pub session: String,
    pub paths: RuntimePaths,
    pub runtime_state: RuntimeState,
    pub services: Vec<ServiceStatus>,
}

/// One service as observed across shutdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShutdownEntry {
    pub id: ServiceId,
    pub before: ServiceState,
    pub after: ServiceState,
}

/// Shutdown snapshot. Entries are in reverse registration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShutdownReport {
    pub entries: Vec<ShutdownEntry>,
    pub runtime_state: RuntimeState,
}

/// Result of a resident run that has finished shutting down.
pub struct RunOutcome {
    pub report: RuntimeReport,
    pub shutdown_report: ShutdownReport,
    pub start_failures: Vec<Error>,
    pub shutdown_failures: Vec<Error>,
}

/// Supervisor that has started and is ready to wait for termination.
pub struct PreparedRuntime {
    session: Session,
    shutdown: ShutdownCoordinator,
    report: RuntimeReport,
}

#[derive(Debug)]
pub enum StartupError {
    Config(wind_config::ConfigError),
    Service(Error),
    Version(VersionError),
    Platform(wind_platform::PlatformError),
}

impl std::fmt::Display for StartupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(error) => write!(formatter, "{error}"),
            Self::Service(error) => write!(formatter, "{error}"),
            Self::Version(error) => {
                write!(
                    formatter,
                    "component=supervisor operation=read_version {error}"
                )
            }
            Self::Platform(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for StartupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::Service(error) => Some(error),
            Self::Version(error) => Some(error),
            Self::Platform(error) => Some(error),
        }
    }
}

impl Session {
    /// Load configuration, resolve the runtime, and start enabled services.
    ///
    /// Startup is sequential. A service that fails to start is recorded and the
    /// remaining services still start.
    pub fn open() -> Result<Self, StartupError> {
        let product = wind_config::load_product_config().map_err(StartupError::Config)?;
        let host = wind_platform::host_runtime();
        let platform = host.platform_info();
        let session = host.session_context().map_err(StartupError::Platform)?;
        let paths = host
            .runtime_paths(product.product().identifier())
            .map_err(StartupError::Platform)?;
        wind_platform::ensure_runtime_paths(&paths).map_err(StartupError::Platform)?;
        wind_core::set_log_file(paths.log_dir.join("wind.log"));
        let supervisor_version =
            Version::parse(env!("CARGO_PKG_VERSION")).map_err(StartupError::Version)?;
        let context = RuntimeContext {
            health_check_interval: product.runtime().health_check_interval(),
            shutdown_timeout: product.runtime().shutdown_timeout(),
            product,
            platform,
            paths,
            session,
        };
        let store = open_store(
            &context.product,
            &context.paths.data_dir,
            context.session.user.user_id.clone(),
        )
        .map_err(StartupError::Service)?;
        let mut session = Self {
            manager: ServiceManager::with_settings(RuntimeSettings {
                shutdown_timeout: context.shutdown_timeout,
            }),
            context,
            supervisor_version,
            store: Arc::clone(&store),
            start_failures: Vec::new(),
            runtime_state: RuntimeState::Initializing,
        };
        session.log(Level::Info, &session.initialized_message());
        session
            .advance(RuntimeState::Starting)
            .map_err(StartupError::Service)?;
        register_builtin_services(
            &mut session.manager,
            &session.context.product,
            &session.context.paths.data_dir,
            &session.context.paths.runtime_dir,
            store,
        )
        .map_err(StartupError::Service)?;
        let session_kind = session.context.session.kind;
        session
            .manager
            .set_permission_provider(Box::new(HostPermissionProvider::new(session_kind)));
        session
            .manager
            .validate_metadata(&ServiceId::ALL)
            .map_err(StartupError::Service)?;
        session.start_failures = session.manager.start_all();
        session
            .advance(RuntimeState::Running)
            .map_err(StartupError::Service)?;
        session.log(Level::Info, "runtime_started");
        session.log_capabilities();
        Ok(session)
    }

    pub fn context(&self) -> &RuntimeContext {
        &self.context
    }

    pub fn product(&self) -> &ProductConfig {
        &self.context.product
    }

    pub fn platform(&self) -> &PlatformInfo {
        &self.context.platform
    }

    pub fn runtime_state(&self) -> RuntimeState {
        self.runtime_state
    }

    pub fn shutdown_timeout(&self) -> Duration {
        self.manager.settings().shutdown_timeout
    }

    pub fn start_failures(&self) -> &[Error] {
        &self.start_failures
    }

    pub fn status(&self, id: ServiceId) -> Option<ServiceStatus> {
        self.manager.get(id)
    }

    /// Current capability and permission observations for running services.
    ///
    /// Each call checks the provider again. Nothing is cached.
    pub fn capability_diagnostics(&self) -> Vec<CapabilityObservation> {
        self.manager.capability_reports()
    }

    pub fn runtime_report(&self) -> RuntimeReport {
        RuntimeReport {
            display_name: self.context.product.product().display_name().to_string(),
            product_version: self.context.product.product().version(),
            platform_name: self
                .context
                .platform
                .operating_system
                .display_name()
                .to_string(),
            architecture: self.context.platform.architecture.as_str().to_string(),
            user: user_label(&self.context.session),
            session: self.context.session.kind.to_string(),
            paths: self.context.paths.clone(),
            runtime_state: self.runtime_state,
            services: self.manager.list(),
        }
    }

    /// Read service health and log degraded or unhealthy services.
    ///
    /// This does not restart services and does not change lifecycle state.
    pub fn poll_health(&self) -> HealthPoll {
        let mut poll = HealthPoll {
            healthy: 0,
            degraded: 0,
            unhealthy: 0,
            unknown: 0,
            permission_required: 0,
            unsupported: 0,
        };
        for (id, health) in self.manager.health_all() {
            match health.status {
                HealthStatus::Healthy => poll.healthy += 1,
                HealthStatus::Degraded => poll.degraded += 1,
                HealthStatus::Unhealthy => poll.unhealthy += 1,
                HealthStatus::Unknown => poll.unknown += 1,
                HealthStatus::PermissionRequired => poll.permission_required += 1,
                HealthStatus::Unsupported => poll.unsupported += 1,
            }
            if let Some(event) = health_log_event(health.status) {
                self.log_service(Level::Warn, id, health.status, event);
            }
        }
        self.log(
            Level::Info,
            &format!(
                "health_check healthy={} degraded={} unhealthy={} unknown={} permission_required={} unsupported={}",
                poll.healthy, poll.degraded, poll.unhealthy, poll.unknown, poll.permission_required, poll.unsupported
            ),
        );
        poll
    }

    /// Wait until `shutdown` is requested, then stop services once.
    ///
    /// The wait blocks. A resident process closes tracking intervals on the
    /// UTC 10-second boundary. A one-shot command has already requested
    /// shutdown, so it does not open a tracking interval.
    pub fn reside(&mut self, shutdown: &ShutdownCoordinator) -> (ShutdownReport, Vec<Error>) {
        if !shutdown.is_requested() {
            let mut tracking = self.begin_tracking();
            let health = self.poll_interval();
            loop {
                let now = SystemTime::now();
                let wait = interval::wait_budget(now, tracking.interval_ends_at(), health);
                if wait.is_zero() {
                    if self.close_due(&mut tracking, SystemTime::now(), false) == 0
                        && shutdown.wait_timeout(Duration::from_millis(50))
                    {
                        break;
                    }
                    self.poll_health();
                    let _ = self.manager.maintain_running();
                    continue;
                }
                if shutdown.wait_timeout(wait) {
                    break;
                }
                self.close_due(&mut tracking, SystemTime::now(), false);
                self.poll_health();
                let _ = self.manager.maintain_running();
            }
            self.close_due(&mut tracking, SystemTime::now(), true);
        }
        let already_stopped = !self.can_begin_shutdown();
        let outcome = self.shutdown();
        let duplicates = shutdown.duplicate_count();
        if !already_stopped && duplicates > 0 {
            self.log(Level::Info, "shutdown_already_requested");
        }
        outcome
    }

    /// Stop running services through the service manager.
    ///
    /// A repeated call does not stop services again.
    pub fn shutdown(&mut self) -> (ShutdownReport, Vec<Error>) {
        let before = self.manager.list();
        if !self.can_begin_shutdown() {
            self.log(Level::Info, "shutdown_already_requested");
            return (self.shutdown_snapshot(&before), Vec::new());
        }
        if let Err(error) = self.advance(RuntimeState::ShutdownRequested) {
            return (self.shutdown_snapshot(&before), vec![error]);
        }
        self.log(Level::Info, "shutdown_requested");
        if let Err(error) = self.advance(RuntimeState::Stopping) {
            return (self.shutdown_snapshot(&before), vec![error]);
        }
        self.log(Level::Info, "runtime_stopping");
        let mut failures = self.manager.shutdown();
        if let Err(error) = self.advance(RuntimeState::Stopped) {
            failures.push(error);
        } else {
            self.log(Level::Info, "runtime_stopped");
        }
        (self.shutdown_snapshot(&before), failures)
    }

    fn can_begin_shutdown(&self) -> bool {
        matches!(
            self.runtime_state,
            RuntimeState::Initializing | RuntimeState::Starting | RuntimeState::Running
        )
    }

    fn poll_interval(&self) -> Duration {
        let interval = self.context.health_check_interval;
        if interval.is_zero() {
            Duration::from_secs(10)
        } else {
            interval
        }
    }

    fn begin_tracking(&mut self) -> interval::TrackingSession {
        self.manager.release_interval_flush();
        let now = SystemTime::now();
        let mut tracking = interval::TrackingSession::start(now);
        tracking.seed(self.sample(&tracking));
        self.log(
            Level::Info,
            &format!("tracking_started started_at={}", interval::format_utc(now)),
        );
        tracking
    }

    fn close_due(
        &mut self,
        tracking: &mut interval::TrackingSession,
        now: SystemTime,
        shutdown: bool,
    ) -> usize {
        let observed = self.sample(tracking);
        let mut count = 0;
        while let Some(window) = tracking.next_due(now, shutdown) {
            let record = tracking.close_window(&window, &observed);
            self.write_interval(&record);
            count += 1;
        }
        count
    }

    fn sample(&mut self, tracking: &interval::TrackingSession) -> interval::Observation {
        let activity = match self.manager.observe_activity() {
            Ok(report) => interval::ActivityTotals {
                keyboard_events: report.keyboard_events,
                mouse_moves: report.mouse_moves,
                mouse_clicks: report.mouse_clicks,
                mouse_scrolls: report.mouse_scrolls,
            },
            Err(_) => tracking.activity_baseline(),
        };
        let application_id = match self.manager.observe_application() {
            Ok(report) if report.present && !report.application_id.is_empty() => {
                Some(report.application_id)
            }
            Ok(_) => None,
            Err(_) => tracking.application_id(),
        };
        let url = match self.manager.observe_url() {
            Ok(report) if report.present && !report.url.is_empty() => {
                sanitize_stored_url(&report.url).ok()
            }
            Ok(_) => None,
            Err(_) => tracking.url_value(),
        };
        interval::Observation {
            activity,
            application_id,
            url,
        }
    }

    fn write_interval(&mut self, record: &interval::IntervalRecord) {
        let draft = IntervalDraft {
            started_at: record.started_at,
            ended_at: record.ended_at,
            activity: Some(ActivityCounts {
                keyboard_events: record.activity.keyboard_events,
                mouse_moves: record.activity.mouse_moves,
                mouse_clicks: record.activity.mouse_clicks,
                mouse_scrolls: record.activity.mouse_scrolls,
            }),
            application: record
                .application
                .as_ref()
                .map(|(id, started_at, ended_at)| ApplicationSegment {
                    application_id: id.clone(),
                    started_at: *started_at,
                    ended_at: *ended_at,
                }),
            url: record
                .url
                .as_ref()
                .map(|(url, started_at, ended_at)| UrlSegment {
                    url: url.clone(),
                    started_at: *started_at,
                    ended_at: *ended_at,
                }),
        };
        let interval_id = match self.store.record_interval(draft) {
            Ok(id) => id,
            Err(error) => {
                self.log(
                    Level::Error,
                    &format!("interval_persist_failed error={error}"),
                );
                return;
            }
        };
        self.log(
            Level::Info,
            &format!(
                "interval_closed started_at={} ended_at={} partial={}",
                interval::format_utc(record.started_at),
                interval::format_utc(record.ended_at),
                record.partial
            ),
        );
        if !record.capture_screenshot {
            return;
        }
        let captured = match self.manager.capture_interval_screenshot() {
            Ok(report) => report,
            Err(error) => {
                self.log(
                    Level::Error,
                    &format!("interval_screenshot_failed error={error}"),
                );
                return;
            }
        };
        if let Err(error) = self.store.attach_artifact(
            &interval_id,
            ArtifactRequest {
                kind: ArtifactKind::Screenshot,
                path: PathBuf::from(&captured.artifact),
                captured_at: captured.captured_at,
            },
        ) {
            self.log(
                Level::Error,
                &format!("interval_screenshot_failed error={error}"),
            );
        }
    }

    fn shutdown_snapshot(&self, before: &[ServiceStatus]) -> ShutdownReport {
        let after = self.manager.list();
        let entries = before
            .iter()
            .rev()
            .map(|status| {
                let after_state = after
                    .iter()
                    .find(|service| service.descriptor.id == status.descriptor.id)
                    .map(|service| service.state)
                    .unwrap_or(status.state);
                ShutdownEntry {
                    id: status.descriptor.id,
                    before: status.state,
                    after: after_state,
                }
            })
            .collect();
        ShutdownReport {
            entries,
            runtime_state: self.runtime_state,
        }
    }

    fn advance(&mut self, next: RuntimeState) -> Result<(), Error> {
        match self.runtime_state.transition(next) {
            Ok(state) => {
                self.runtime_state = state;
                Ok(())
            }
            Err(error) => Err(Error::new(
                SUPERVISOR_COMPONENT,
                "runtime_transition",
                error.to_string(),
            )
            .with_state(self.runtime_state.as_str())),
        }
    }

    fn initialized_message(&self) -> String {
        format!(
            "runtime_initialized display_name='{}' identifier={} session={} config={} data={} logs={} cache={} runtime={}",
            self.context.product.product().display_name(),
            self.context.product.product().identifier(),
            self.context.session.kind,
            self.context.paths.config_dir.display(),
            self.context.paths.data_dir.display(),
            self.context.paths.log_dir.display(),
            self.context.paths.cache_dir.display(),
            self.context.paths.runtime_dir.display(),
        )
    }

    fn log_capabilities(&self) {
        for observation in self.manager.capability_reports() {
            let message = format!(
                "capability_checked capability={} required={} permission={} capability_status={}",
                observation.capability.as_str(),
                observation.required,
                observation.permission.as_str(),
                observation.capability_status.as_str()
            );
            self.log_service(
                Level::Info,
                observation.service_id,
                observation.health,
                &message,
            );
        }
    }

    fn log(&self, level: Level, message: &str) {
        let version = self.supervisor_version.to_string();
        let platform_name = format!(
            "{}-{}",
            self.context.platform.operating_system.as_str(),
            self.context.platform.architecture.as_str()
        );
        let state = self.runtime_state.as_str();
        emit(&LogFields {
            level,
            component: SUPERVISOR_COMPONENT,
            service_id: None,
            version: Some(&version),
            platform: Some(&platform_name),
            message,
            error: None,
            state: Some(state),
        });
    }

    fn log_service(&self, level: Level, id: ServiceId, health: HealthStatus, message: &str) {
        let version = self.supervisor_version.to_string();
        let platform_name = format!(
            "{}-{}",
            self.context.platform.operating_system.as_str(),
            self.context.platform.architecture.as_str()
        );
        let service_id = id.as_str();
        let state = health.as_str();
        emit(&LogFields {
            level,
            component: SUPERVISOR_COMPONENT,
            service_id: Some(service_id),
            version: Some(&version),
            platform: Some(&platform_name),
            message,
            error: None,
            state: Some(state),
        });
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.can_begin_shutdown() {
            let _ = self.shutdown();
        }
    }
}

impl PreparedRuntime {
    pub fn report(&self) -> &RuntimeReport {
        &self.report
    }

    /// Ask the resident loop to stop. `wait` then shuts services down.
    pub fn request_shutdown(&self) {
        let _ = self.shutdown.request();
    }

    /// Capture one screenshot through the running screenshot service.
    pub fn capture_screenshot(&mut self) -> wind_core::Result<CaptureReport> {
        self.session.manager.capture_screenshot()
    }

    /// Start one video recording through the running video service.
    pub fn start_recording(&mut self) -> wind_core::Result<()> {
        self.session.manager.start_recording()
    }

    /// Finalize the current video recording.
    pub fn stop_recording(&mut self) -> wind_core::Result<CaptureReport> {
        self.session.manager.stop_recording()
    }

    /// Query the foreground application.
    pub fn current_application(&mut self) -> wind_core::Result<wind_core::ApplicationReport> {
        self.session.manager.current_application()
    }

    /// Read cumulative activity counters.
    pub fn activity_snapshot(&mut self) -> wind_core::Result<wind_core::ActivityReport> {
        self.session.manager.activity_snapshot()
    }

    /// Query the latest sanitized browser URL.
    pub fn current_url(&mut self) -> wind_core::Result<wind_core::UrlReport> {
        self.session.manager.current_url()
    }

    /// Current permission answers. This checks again and does not cache a grant.
    pub fn permission_report(&self) -> String {
        let lines =
            Capability::ALL.map(|capability| permission_line(&self.session.manager, capability));
        format_permission_report(&lines)
    }

    /// Open the settings surface, then check the permission again.
    pub fn open_permission(
        &self,
        capability: Capability,
    ) -> std::result::Result<String, wind_core::PermissionError> {
        let before = self.session.manager.permission_status(capability).state;
        let opened = self.session.manager.open_settings(capability);
        let after = self.session.manager.permission_status(capability).state;
        let mut text = match &opened {
            Ok(()) => String::from("Open settings: Opened\n"),
            Err(error) => format!("Open settings: {}\n{error}\n", error.outcome()),
        };
        text.push_str(&format!("Permission: {}\n", after.as_str()));
        if before == after {
            text.push_str("Opening settings did not grant the permission.\n");
        }
        match opened {
            Ok(())
            | Err(wind_core::PermissionError::Unsupported { .. })
            | Err(wind_core::PermissionError::Unavailable { .. }) => Ok(text),
            Err(error) => Err(error),
        }
    }

    /// Block until termination is requested, then stop services and return.
    pub fn wait(mut self) -> RunOutcome {
        let (shutdown_report, shutdown_failures) = self.session.reside(&self.shutdown);
        let start_failures = std::mem::take(&mut self.session.start_failures);
        RunOutcome {
            report: self.report,
            shutdown_report,
            start_failures,
            shutdown_failures,
        }
    }
}

/// Start services and install the termination notifier.
///
/// The notifier only calls [`ShutdownCoordinator::request`]. The returned
/// runtime stays alive until [`PreparedRuntime::wait`] observes that request.
pub fn prepare() -> Result<PreparedRuntime, StartupError> {
    let mut session = Session::open()?;
    let shutdown = ShutdownCoordinator::new();
    let notify = shutdown.clone();
    if let Err(error) = wind_platform::install_termination_handler(move || {
        let _ = notify.request();
    }) {
        let _ = session.shutdown();
        return Err(StartupError::Platform(error));
    }
    let report = session.runtime_report();
    Ok(PreparedRuntime {
        session,
        shutdown,
        report,
    })
}

/// Human-readable runtime report. Values come from the snapshot.
pub fn format_report(report: &RuntimeReport) -> String {
    let mut lines = Vec::with_capacity(16 + report.services.len());
    lines.push(format!("{} Agent", report.display_name));
    lines.push(format!("Version: {}", report.product_version));
    lines.push(format!("Platform: {}", report.platform_name));
    lines.push(format!("Architecture: {}", report.architecture));
    lines.push(String::new());
    lines.push("Runtime:".to_string());
    lines.push(format!("  User: {}", report.user));
    lines.push(format!("  Session: {}", report.session));
    lines.push(format!("  Config: {}", report.paths.config_dir.display()));
    lines.push(format!("  Data: {}", report.paths.data_dir.display()));
    lines.push(format!("  Logs: {}", report.paths.log_dir.display()));
    lines.push(format!("  Cache: {}", report.paths.cache_dir.display()));
    lines.push(String::new());
    lines.push("Services:".to_string());
    for service in &report.services {
        lines.push(format_service_line(service));
    }
    lines.push(String::new());
    lines.push(format!("Runtime state: {}", report.runtime_state.as_str()));
    lines.join("\n") + "\n"
}

/// Human-readable shutdown summary.
pub fn format_shutdown(report: &ShutdownReport) -> String {
    let mut lines = Vec::with_capacity(4 + report.entries.len());
    lines.push("Shutdown requested".to_string());
    lines.push("Stopping services".to_string());
    for entry in &report.entries {
        lines.push(format_shutdown_line(entry));
    }
    lines.push(String::new());
    lines.push(format!("Runtime state: {}", report.runtime_state.as_str()));
    lines.join("\n") + "\n"
}

fn format_service_line(service: &ServiceStatus) -> String {
    if service.state == ServiceState::Disabled {
        format!("  {:<12} Disabled", service.descriptor.id.as_str())
    } else {
        format!(
            "  {:<12} {} / {}",
            service.descriptor.id.as_str(),
            service.state.as_str(),
            service.health.status.as_str()
        )
    }
}

fn format_shutdown_line(entry: &ShutdownEntry) -> String {
    let detail = if entry.before == ServiceState::Disabled {
        "Disabled".to_string()
    } else if entry.before == ServiceState::Running && entry.after == ServiceState::Stopped {
        "Stopping → Stopped".to_string()
    } else if entry.before == ServiceState::Running {
        format!("Stopping → {}", entry.after.as_str())
    } else if entry.before == entry.after {
        entry.before.as_str().to_string()
    } else {
        format!("{} → {}", entry.before.as_str(), entry.after.as_str())
    };
    format!("  {:<12} {detail}", entry.id.as_str())
}

fn user_label(session: &SessionContext) -> String {
    session
        .user
        .username
        .clone()
        .unwrap_or_else(|| "unknown".to_string())
}

fn health_log_event(status: HealthStatus) -> Option<&'static str> {
    match status {
        HealthStatus::Degraded => Some("service_degraded"),
        HealthStatus::Unhealthy => Some("service_unhealthy"),
        HealthStatus::PermissionRequired => Some("permission_required"),
        HealthStatus::Unsupported => Some("capability_unsupported"),
        HealthStatus::Healthy | HealthStatus::Unknown => None,
    }
}

/// One permission row for the CLI report.
pub struct PermissionLine {
    pub capability: Capability,
    pub capability_status: CapabilityStatus,
    pub permission: PermissionState,
    pub reason: Option<String>,
    pub service_health: Option<HealthStatus>,
    pub settings: bool,
}

/// User-facing permission report. Opening settings is not implied.
pub fn format_permission_report(lines: &[PermissionLine]) -> String {
    let mut text = String::from("Wind Permissions\n");
    for line in lines {
        let copy = wind_core::permission_copy(line.capability);
        let capability_status = match line.service_health {
            Some(health) if line.permission == PermissionState::Unknown => {
                capability_from_health(health)
            }
            _ => line.capability_status,
        };
        let status = wind_core::user_permission_status(line.permission, capability_status);
        text.push('\n');
        text.push_str(copy.title);
        text.push('\n');
        text.push_str(&format!("  Capability: {}\n", capability_status.as_str()));
        text.push_str(&format!("  Permission: {}\n", line.permission.as_str()));
        text.push_str(&format!("  Status: {}\n", status.label()));
        if let Some(reason) = line.reason.as_deref() {
            text.push_str("  ");
            text.push_str(reason);
            text.push('\n');
        }
        text.push_str("  ");
        text.push_str(copy.privacy);
        text.push('\n');
        if status.offers_settings() {
            text.push_str("  ");
            text.push_str(copy.requirement);
            text.push('\n');
            if line.settings {
                text.push_str("  Action: Open System Settings\n");
            }
        }
    }
    text
}

fn permission_line(
    manager: &wind_service_manager::ServiceManager,
    capability: Capability,
) -> PermissionLine {
    let status = manager.permission_status(capability);
    let service_health = manager
        .capability_reports()
        .into_iter()
        .find(|item| item.capability == capability)
        .map(|item| item.health);
    PermissionLine {
        capability,
        capability_status: manager.permission_capability_status(capability),
        permission: status.state,
        reason: status.reason,
        service_health,
        settings: manager.can_open_settings(capability),
    }
}

fn capability_from_health(health: HealthStatus) -> CapabilityStatus {
    match health {
        HealthStatus::Healthy => CapabilityStatus::Available,
        HealthStatus::PermissionRequired => CapabilityStatus::PermissionRequired,
        HealthStatus::Unsupported => CapabilityStatus::Unsupported,
        HealthStatus::Degraded | HealthStatus::Unhealthy => CapabilityStatus::Unavailable,
        HealthStatus::Unknown => CapabilityStatus::Unknown,
    }
}

/// Detailed capability report. The resident stdout report does not print this.
pub fn format_capability_diagnostics(observations: &[CapabilityObservation]) -> String {
    let mut lines = Vec::new();
    let mut current = None;
    for observation in observations {
        if current != Some(observation.service_id) {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push(format!("  {}", observation.service_id.as_str()));
            lines.push(format!("    Lifecycle: {}", observation.lifecycle.as_str()));
            lines.push(format!("    Health: {}", observation.health.as_str()));
            current = Some(observation.service_id);
        }
        lines.push(format!(
            "    Capability: {}",
            observation.capability.as_str()
        ));
        lines.push(format!(
            "    Permission: {}",
            observation.permission.as_str()
        ));
    }
    if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n") + "\n"
    }
}

fn sanitize_stored_url(raw: &str) -> Result<String, String> {
    wind_url_tracker::sanitize_http_url(raw).map_err(|_| "url was rejected".to_string())
}

fn open_store(
    product: &ProductConfig,
    data_dir: &Path,
    user_id: Option<String>,
) -> Result<Arc<dyn LocalStore>, Error> {
    let opened = SqliteStore::open(
        data_dir,
        StoreOptions {
            max_local_storage_bytes: DEFAULT_MAX_LOCAL_STORAGE_BYTES,
            config_version: product.product().version().to_string(),
            user_id,
            sanitize_url: sanitize_stored_url,
        },
    )
    .map_err(|error| Error::new("storage", "open", error.to_string()).with_service("storage"))?;
    Ok(Arc::new(opened))
}

fn register_builtin_services(
    manager: &mut ServiceManager,
    product: &ProductConfig,
    data_dir: &Path,
    runtime_dir: &Path,
    store: Arc<dyn LocalStore>,
) -> Result<(), Error> {
    let screenshot = wind_screenshot::create_with_store(
        wind_screenshot::screenshot_directory(data_dir),
        host_screenshot_provider(),
        Some(Arc::clone(&store)),
    )?;
    let screenshot_enabled = product.services().enabled(screenshot.id());
    manager.register(Box::new(screenshot), screenshot_enabled)?;
    let activity =
        wind_activity::create_with_store(host_activity_provider(), Some(Arc::clone(&store)))?;
    let activity_enabled = product.services().enabled(activity.id());
    manager.register(Box::new(activity), activity_enabled)?;
    let manifest = manifest::ServiceManifest::load(runtime_dir)?;
    let app_enabled = product.services().enabled(ServiceId::AppTracker);
    if let Some(install) = manifest
        .install(ServiceId::AppTracker)
        .filter(|install| install.enabled && app_enabled)
    {
        let hosted = hosted::HostedAppTracker::prepare(install, Some(Arc::clone(&store)))?;
        manager.register(Box::new(hosted), true)?;
    } else {
        let app_tracker =
            wind_app_tracker::create_with_store(host_app_tracker(), Some(Arc::clone(&store)))?;
        manager.register(Box::new(app_tracker), app_enabled)?;
    }
    let url_tracker =
        wind_url_tracker::create_with_store(host_url_tracker(), Some(Arc::clone(&store)))?;
    let url_enabled = product.services().enabled(url_tracker.id());
    manager.register(Box::new(url_tracker), url_enabled)?;
    let video = wind_video::create_with_store(
        wind_video::video_directory(data_dir),
        host_video_provider(),
        Some(Arc::clone(&store)),
    )?;
    let video_enabled = product.services().enabled(video.id());
    manager.register(Box::new(video), video_enabled)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        format_capability_diagnostics, format_permission_report, format_report, format_shutdown,
        health_log_event, PermissionLine, RuntimeState, Session, ShutdownCoordinator,
    };
    use std::time::Duration;
    use wind_core::{
        technical_id, Capability, CapabilityStatus, HealthStatus, PermissionState, Service,
        ServiceId, ServiceState, SUPERVISOR_COMPONENT,
    };
    use wind_service_manager::CapabilityObservation;

    #[test]
    fn supervisor_starts_enabled_services_and_shuts_down() {
        let mut session = Session::open().expect("supervisor should open from the workspace");
        assert!(session.start_failures().is_empty());
        assert_eq!(session.runtime_state(), RuntimeState::Running);
        assert_eq!(session.shutdown_timeout(), Duration::from_secs(5));
        assert_eq!(
            session.context().health_check_interval,
            Duration::from_secs(10)
        );
        let report = session.runtime_report();
        let text = format_report(&report);
        let expected_title = format!("{} Agent", session.product().product().display_name());
        assert_eq!(text.lines().next(), Some(expected_title.as_str()));
        assert!(text.contains(&format!("Version: {}", report.product_version)));
        assert!(text.contains(&format!("Platform: {}", report.platform_name)));
        assert!(text.contains(&format!("Architecture: {}", report.architecture)));
        assert!(text.contains(&format!("User: {}", report.user)));
        assert!(text.contains(&format!("Session: {}", report.session)));
        assert!(text.contains(&format!("Config: {}", report.paths.config_dir.display())));
        assert!(report.paths.config_dir.is_absolute());
        assert!(report.paths.data_dir.is_absolute());
        assert!(report.paths.log_dir.is_absolute());
        assert!(report.paths.cache_dir.is_absolute());
        assert!(report.paths.runtime_dir.is_absolute());
        let identifier = session.product().product().identifier();
        assert!(report
            .paths
            .config_dir
            .components()
            .any(|component| component.as_os_str() == identifier));
        assert!(text.contains("Runtime state: Running"));
        assert!(!text.contains("Permission:"));
        let diagnostics = session.capability_diagnostics();
        assert!(diagnostics
            .iter()
            .any(|item| item.capability == Capability::ScreenCapture));
        assert!(diagnostics
            .iter()
            .all(|item| item.lifecycle == ServiceState::Running));
        assert!(!format_capability_diagnostics(&diagnostics).is_empty());

        let ids: Vec<_> = report
            .services
            .iter()
            .map(|service| service.descriptor.id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "screenshot",
                "activity",
                "app_tracker",
                "url_tracker",
                "video"
            ]
        );

        for id in [
            ServiceId::Screenshot,
            ServiceId::Activity,
            ServiceId::AppTracker,
            ServiceId::UrlTracker,
        ] {
            let status = session.status(id).expect("service");
            assert_eq!(status.state, ServiceState::Running);
            assert_ne!(status.state, ServiceState::Failed);
            assert_ne!(status.health.status, HealthStatus::Unhealthy);
            assert!(status.descriptor.enabled);
            assert!(text.contains(&format!(
                "{} / {}",
                status.state.as_str(),
                status.health.status.as_str()
            )));
            assert_eq!(
                session.product().product().service_technical_id(id),
                technical_id(
                    session.product().product().identifier(),
                    id.technical_component()
                )
            );
        }

        let video = session.status(ServiceId::Video).expect("video");
        assert_eq!(video.state, ServiceState::Disabled);
        assert_eq!(video.health.status, HealthStatus::Unknown);
        assert!(!video.descriptor.enabled);
        assert!(text.contains("Disabled"));
        assert_eq!(
            session.product().product().supervisor_technical_id(),
            technical_id(
                session.product().product().identifier(),
                SUPERVISOR_COMPONENT
            )
        );

        let (shutdown_report, shutdown_failures) = session.shutdown();
        assert!(shutdown_failures.is_empty(), "{shutdown_failures:?}");
        assert_eq!(session.runtime_state(), RuntimeState::Stopped);
        assert_eq!(
            shutdown_report
                .entries
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            [
                "video",
                "url_tracker",
                "app_tracker",
                "activity",
                "screenshot"
            ]
        );
        let shutdown_text = format_shutdown(&shutdown_report);
        assert!(shutdown_text.contains("Shutdown requested"));
        assert!(shutdown_text.contains("Stopping → Stopped"));
        assert!(shutdown_text.contains("Runtime state: Stopped"));
        for id in [
            ServiceId::Screenshot,
            ServiceId::Activity,
            ServiceId::AppTracker,
            ServiceId::UrlTracker,
        ] {
            assert_eq!(
                session.status(id).expect("service").state,
                ServiceState::Stopped
            );
        }
        assert_eq!(
            session.status(ServiceId::Video).expect("video").state,
            ServiceState::Disabled
        );

        let (repeated, repeated_failures) = session.shutdown();
        assert!(repeated_failures.is_empty(), "{repeated_failures:?}");
        assert_eq!(repeated.runtime_state, RuntimeState::Stopped);
        assert_eq!(
            session
                .status(ServiceId::Screenshot)
                .expect("service")
                .state,
            ServiceState::Stopped
        );
    }

    #[test]
    fn health_poll_does_not_change_service_state() {
        let session = Session::open().expect("supervisor should open");
        let before: Vec<_> = ServiceId::ALL
            .iter()
            .map(|id| session.status(*id).expect("service").state)
            .collect();
        let poll = session.poll_health();
        let after: Vec<_> = ServiceId::ALL
            .iter()
            .map(|id| session.status(*id).expect("service").state)
            .collect();
        assert_eq!(before, after);
        let healths: Vec<_> = ServiceId::ALL
            .iter()
            .map(|id| session.status(*id).expect("service").health.status)
            .collect();
        let count = |status: HealthStatus| healths.iter().filter(|item| **item == status).count();
        assert_eq!(poll.healthy, count(HealthStatus::Healthy));
        assert_eq!(poll.degraded, count(HealthStatus::Degraded));
        assert_eq!(poll.unhealthy, 0);
        assert_eq!(poll.unknown, count(HealthStatus::Unknown));
        assert_eq!(
            poll.permission_required,
            count(HealthStatus::PermissionRequired)
        );
        assert_eq!(poll.unsupported, count(HealthStatus::Unsupported));
        assert!(poll.unknown >= 1);
        assert_eq!(session.runtime_state(), RuntimeState::Running);
        assert_eq!(
            health_log_event(HealthStatus::Unhealthy),
            Some("service_unhealthy")
        );
        assert_eq!(
            health_log_event(HealthStatus::Degraded),
            Some("service_degraded")
        );
        assert_eq!(health_log_event(HealthStatus::Healthy), None);
        assert_eq!(health_log_event(HealthStatus::Unknown), None);
        assert_eq!(
            health_log_event(HealthStatus::PermissionRequired),
            Some("permission_required")
        );
        assert_eq!(
            health_log_event(HealthStatus::Unsupported),
            Some("capability_unsupported")
        );
    }

    #[test]
    fn services_declare_expected_capabilities() {
        let screenshot = wind_screenshot::create(
            std::env::temp_dir().join("wind-screenshot-declare"),
            Box::new(wind_screenshot::FakeScreenshotProvider::new(
                wind_screenshot::FakeOutcome::Unsupported,
            )),
        )
        .unwrap();
        let activity = wind_activity::create(Box::new(wind_activity::FakeActivityProvider::new(
            wind_activity::FakeActivityOutcome::Unsupported,
        )))
        .unwrap();
        let app_tracker = wind_app_tracker::create(Box::new(
            wind_app_tracker::FakeAppTracker::new(wind_app_tracker::FakeAppOutcome::Unsupported),
        ))
        .unwrap();
        let url_tracker = wind_url_tracker::create(Box::new(
            wind_url_tracker::FakeUrlTracker::new(wind_url_tracker::FakeUrlOutcome::Unsupported),
        ))
        .unwrap();
        let video = wind_video::create(
            std::env::temp_dir().join("wind-video-declare"),
            Box::new(wind_video::FakeVideoProvider::new(
                wind_video::FakeOutcome::Unsupported,
            )),
        )
        .unwrap();
        let expectations: [(&dyn Service, Capability); 5] = [
            (&screenshot, Capability::ScreenCapture),
            (&activity, Capability::InputMonitoring),
            (&app_tracker, Capability::ActiveApplication),
            (&url_tracker, Capability::BrowserUrl),
            (&video, Capability::VideoCapture),
        ];
        for (service, capability) in expectations {
            assert_eq!(service.capabilities().len(), 1);
            assert_eq!(service.capabilities()[0].capability, capability);
            assert!(service.capabilities()[0].required);
            assert!(!service.configuration().monitoring_enabled);
        }
    }

    #[test]
    fn capability_diagnostics_keep_permission_separate_from_failure() {
        let text = format_capability_diagnostics(&[CapabilityObservation {
            service_id: ServiceId::Screenshot,
            lifecycle: ServiceState::Running,
            health: HealthStatus::PermissionRequired,
            capability: Capability::ScreenCapture,
            required: true,
            capability_status: CapabilityStatus::PermissionRequired,
            permission: PermissionState::NotDetermined,
            reason: None,
        }]);
        assert!(text.contains("Lifecycle: Running"));
        assert!(text.contains("Health: PermissionRequired"));
        assert!(text.contains("Capability: screen_capture"));
        assert!(text.contains("Permission: NotDetermined"));
        assert!(!text.contains("Failed"));
    }

    #[test]
    fn permission_report_describes_action_without_inventing_a_grant() {
        let available = format_permission_report(&[PermissionLine {
            capability: Capability::ScreenCapture,
            capability_status: CapabilityStatus::Unknown,
            permission: PermissionState::Unknown,
            reason: Some("Windows does not use a separate screen-capture permission.".to_string()),
            service_health: Some(HealthStatus::Healthy),
            settings: false,
        }]);
        assert!(available.contains("Status: Available"));
        assert!(available.contains("Permission: Unknown"));
        assert!(!available.contains("Action:"));
        assert!(!available.contains("Granted"));

        let required = format_permission_report(&[PermissionLine {
            capability: Capability::ScreenCapture,
            capability_status: CapabilityStatus::PermissionRequired,
            permission: PermissionState::NotDetermined,
            reason: None,
            service_health: None,
            settings: true,
        }]);
        assert!(required.contains("Status: Permission required"));
        assert!(required.contains("Wind needs screen-capture access to take screenshots."));
        assert!(required.contains("Action: Open System Settings"));

        let activity = format_permission_report(&[PermissionLine {
            capability: Capability::InputMonitoring,
            capability_status: CapabilityStatus::Available,
            permission: PermissionState::Granted,
            reason: None,
            service_health: Some(HealthStatus::Healthy),
            settings: true,
        }]);
        assert!(activity.contains("does not store the characters you type"));
        assert!(!activity.contains("Action:"));

        let url = format_permission_report(&[PermissionLine {
            capability: Capability::BrowserUrl,
            capability_status: CapabilityStatus::PermissionRequired,
            permission: PermissionState::Denied,
            reason: None,
            service_health: None,
            settings: true,
        }]);
        assert!(url.contains("does not read browser history"));
        assert!(url.contains("Action: Open System Settings"));
    }

    #[test]
    fn resident_shutdown_stops_services_once() {
        let mut session = Session::open().expect("supervisor should open");
        let shutdown = ShutdownCoordinator::new();
        assert!(shutdown.request());
        assert!(!shutdown.request());
        let (report, failures) = session.reside(&shutdown);
        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(session.runtime_state(), RuntimeState::Stopped);
        assert_eq!(report.runtime_state, RuntimeState::Stopped);
        assert_eq!(report.entries[0].before, ServiceState::Disabled);
        assert_eq!(report.entries[0].after, ServiceState::Disabled);
        for entry in report.entries.iter().skip(1) {
            assert_eq!(entry.before, ServiceState::Running);
            assert_eq!(entry.after, ServiceState::Stopped);
        }
        let (again, again_failures) = session.reside(&shutdown);
        assert!(again_failures.is_empty(), "{again_failures:?}");
        assert_eq!(again.runtime_state, RuntimeState::Stopped);
        assert_eq!(
            session.status(ServiceId::Activity).expect("service").state,
            ServiceState::Stopped
        );
    }
}
