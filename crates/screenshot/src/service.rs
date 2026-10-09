//! Screenshot service.
//!
//! Capture runs on one worker thread and only when `capture_once` is called.
//! A missing permission leaves the service able to start.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use wind_core::{
    emit, Capability, CapabilityRequirement, CapabilityStatus, CaptureReport, Error, ExecutionMode,
    Health, HealthStatus, Level, LogFields, Readiness, Result, Service, ServiceConfiguration,
    ServiceId, Version,
};

use wind_storage::{ArtifactKind, ArtifactRequest, LocalStore};

use crate::contract::{ScreenshotError, ScreenshotProvider, ScreenshotRequest};
use crate::storage::{ScreenshotStorage, StoredScreenshot};

const REQUIREMENTS: &[CapabilityRequirement] = &[CapabilityRequirement {
    capability: Capability::ScreenCapture,
    required: true,
}];

enum WorkerRequest {
    Capture {
        reply: Sender<std::result::Result<StoredScreenshot, ScreenshotError>>,
        register: bool,
    },
    Shutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HealthNote {
    Clear,
    Degraded,
    Unhealthy,
}

struct Inner {
    provider: Arc<dyn ScreenshotProvider>,
    storage: ScreenshotStorage,
    version: Version,
    started: AtomicBool,
    stopping: AtomicBool,
    busy: AtomicBool,
    note: Mutex<HealthNote>,
    sender: Mutex<Option<mpsc::Sender<WorkerRequest>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    catalog: Option<Arc<dyn LocalStore>>,
}

/// In-process screenshot service.
///
/// `monitoring_enabled` stays false. Nothing is captured until
/// [`ScreenshotService::capture_once`].
pub struct ScreenshotService {
    inner: Arc<Inner>,
}

impl ScreenshotService {
    pub(crate) fn new(
        version: Version,
        storage: ScreenshotStorage,
        provider: Arc<dyn ScreenshotProvider>,
        catalog: Option<Arc<dyn LocalStore>>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                provider,
                storage,
                version,
                started: AtomicBool::new(false),
                stopping: AtomicBool::new(false),
                busy: AtomicBool::new(false),
                note: Mutex::new(HealthNote::Clear),
                sender: Mutex::new(None),
                worker: Mutex::new(None),
                catalog,
            }),
        }
    }

    /// Capture one full display and store it. A second caller is rejected.
    pub fn capture_once(&self) -> std::result::Result<StoredScreenshot, ScreenshotError> {
        self.capture(true)
    }

    /// Store one screenshot file and leave interval registration to the caller.
    pub fn capture_for_interval(&self) -> std::result::Result<StoredScreenshot, ScreenshotError> {
        self.capture(false)
    }

    fn capture(&self, register: bool) -> std::result::Result<StoredScreenshot, ScreenshotError> {
        if !self.inner.started.load(Ordering::Acquire)
            || self.inner.stopping.load(Ordering::Acquire)
        {
            return Err(ScreenshotError::Stopped);
        }
        if self
            .inner
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ScreenshotError::AlreadyInProgress);
        }
        let (reply_tx, reply_rx) = mpsc::channel();
        let sent = self
            .inner
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|sender| {
                sender.send(WorkerRequest::Capture {
                    reply: reply_tx,
                    register,
                })
            });
        if !matches!(sent, Some(Ok(()))) {
            self.inner.busy.store(false, Ordering::Release);
            return Err(ScreenshotError::Stopped);
        }
        match reply_rx.recv() {
            Ok(result) => result,
            Err(_) => {
                self.inner.busy.store(false, Ordering::Release);
                Err(ScreenshotError::Stopped)
            }
        }
    }

    fn shutdown_worker(&self) {
        self.inner.stopping.store(true, Ordering::Release);
        if let Some(sender) = self
            .inner
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = sender.send(WorkerRequest::Shutdown);
        }
        if let Some(worker) = self
            .inner
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = worker.join();
        }
        self.inner.started.store(false, Ordering::Release);
        self.inner.busy.store(false, Ordering::Release);
    }
}

impl Drop for ScreenshotService {
    fn drop(&mut self) {
        self.shutdown_worker();
    }
}

impl Inner {
    fn run(self: &Arc<Self>, receiver: Receiver<WorkerRequest>) {
        while let Ok(request) = receiver.recv() {
            match request {
                WorkerRequest::Shutdown => break,
                WorkerRequest::Capture { reply, register } => {
                    let result = if self.stopping.load(Ordering::Acquire) {
                        Err(ScreenshotError::Stopped)
                    } else {
                        catch_unwind(AssertUnwindSafe(|| self.capture_and_store(register)))
                            .unwrap_or_else(|_| {
                                Err(ScreenshotError::CaptureFailed {
                                    reason: "screenshot worker panicked".to_string(),
                                })
                            })
                    };
                    self.record(&result);
                    self.busy.store(false, Ordering::Release);
                    let _ = reply.send(result);
                }
            }
        }
    }

    fn capture_and_store(
        &self,
        register: bool,
    ) -> std::result::Result<StoredScreenshot, ScreenshotError> {
        let status = self.provider.capability_status();
        if status != CapabilityStatus::Available {
            return Err(status_error(status));
        }
        log_event(self.version, "screenshot_capture_started");
        let started = Instant::now();
        let image = self.provider.capture(&ScreenshotRequest::FullDisplay)?;
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let width = image.width;
        let height = image.height;
        let byte_len = image.data.len();
        let stored = self.storage.store(image)?;
        if register {
            if let Some(catalog) = &self.catalog {
                catalog
                    .register_artifact(ArtifactRequest {
                        kind: ArtifactKind::Screenshot,
                        path: stored.path.clone(),
                        captured_at: stored.captured_at,
                    })
                    .map_err(|_| ScreenshotError::StorageFailed {
                        reason: "screenshot metadata could not be saved".to_string(),
                    })?;
            }
        }
        log_completed(self.version, width, height, byte_len, duration_ms);
        Ok(stored)
    }

    fn record(&self, result: &std::result::Result<StoredScreenshot, ScreenshotError>) {
        let note = match result {
            Ok(_) => HealthNote::Clear,
            Err(ScreenshotError::CaptureFailed { .. }) => HealthNote::Unhealthy,
            Err(
                ScreenshotError::EncodingFailed { .. }
                | ScreenshotError::StorageFailed { .. }
                | ScreenshotError::ProviderUnavailable { .. }
                | ScreenshotError::NoDisplayAvailable { .. },
            ) => HealthNote::Degraded,
            Err(
                ScreenshotError::PermissionRequired { .. }
                | ScreenshotError::Unsupported { .. }
                | ScreenshotError::AlreadyInProgress
                | ScreenshotError::Stopped,
            ) => HealthNote::Clear,
        };
        *self
            .note
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = note;
        if let Err(error) = result {
            let text = error.to_string();
            emit(&LogFields {
                level: Level::Error,
                component: "screenshot",
                service_id: Some(ServiceId::Screenshot.as_str()),
                version: None,
                platform: None,
                message: "screenshot_capture_failed",
                error: Some(&text),
                state: Some("Running"),
            });
        }
    }

    fn health_parts(&self) -> (HealthStatus, &'static str) {
        if !self.started.load(Ordering::Acquire) {
            return (HealthStatus::Unknown, "screenshot service is not running");
        }
        match self.provider.capability_status() {
            CapabilityStatus::Unsupported => (
                HealthStatus::Unsupported,
                "screen capture is not implemented on this operating system",
            ),
            CapabilityStatus::PermissionRequired => (
                HealthStatus::PermissionRequired,
                "screen capture permission is required",
            ),
            CapabilityStatus::Unknown => {
                (HealthStatus::Unknown, "screen capture status is unknown")
            }
            CapabilityStatus::Unavailable => (
                HealthStatus::Degraded,
                "screen capture is temporarily unavailable",
            ),
            CapabilityStatus::Available => {
                match *self
                    .note
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                {
                    HealthNote::Unhealthy => (HealthStatus::Unhealthy, "screenshot capture failed"),
                    HealthNote::Degraded => (
                        HealthStatus::Degraded,
                        "screenshot storage or encoding failed",
                    ),
                    HealthNote::Clear => (
                        HealthStatus::Healthy,
                        "screen capture is available; a screenshot is taken only when requested",
                    ),
                }
            }
        }
    }
}

fn status_error(status: CapabilityStatus) -> ScreenshotError {
    match status {
        CapabilityStatus::PermissionRequired => ScreenshotError::PermissionRequired {
            state: wind_core::PermissionState::NotDetermined,
            reason: "screen capture permission is required".to_string(),
        },
        CapabilityStatus::Unsupported => ScreenshotError::Unsupported {
            reason: "screen capture is not implemented on this operating system".to_string(),
        },
        CapabilityStatus::Unavailable => ScreenshotError::ProviderUnavailable {
            reason: "screen capture is temporarily unavailable".to_string(),
        },
        CapabilityStatus::Unknown => ScreenshotError::ProviderUnavailable {
            reason: "screen capture status is unknown".to_string(),
        },
        CapabilityStatus::Available => ScreenshotError::CaptureFailed {
            reason: "screen capture status changed during capture".to_string(),
        },
    }
}

fn log_event(version: Version, message: &str) {
    let version = version.to_string();
    emit(&LogFields {
        level: Level::Info,
        component: "screenshot",
        service_id: Some(ServiceId::Screenshot.as_str()),
        version: Some(&version),
        platform: None,
        message,
        error: None,
        state: Some("Running"),
    });
}

fn log_completed(version: Version, width: u32, height: u32, byte_len: usize, duration_ms: u64) {
    let message = format!(
        "screenshot_capture_completed width={width} height={height} bytes={byte_len} duration_ms={duration_ms}"
    );
    log_event(version, &message);
}

fn start_error(message: impl Into<String>) -> Error {
    Error::new("screenshot", "start", message).with_service(ServiceId::Screenshot.as_str())
}

impl Service for ScreenshotService {
    fn id(&self) -> ServiceId {
        ServiceId::Screenshot
    }

    fn name(&self) -> &str {
        ServiceId::Screenshot.display_name()
    }

    fn version(&self) -> Version {
        self.inner.version
    }

    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::InProcess
    }

    fn start(&mut self) -> Result<()> {
        if self.inner.started.load(Ordering::Acquire) {
            return Err(start_error("screenshot service is already running"));
        }
        self.inner.stopping.store(false, Ordering::Release);
        *self
            .inner
            .note
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = HealthNote::Clear;
        let (sender, receiver) = mpsc::channel();
        let inner = Arc::clone(&self.inner);
        let worker = thread::Builder::new()
            .name("wind-screenshot".to_string())
            .spawn(move || inner.run(receiver))
            .map_err(|error| start_error(format!("screenshot worker could not start: {error}")))?;
        *self
            .inner
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sender);
        *self
            .inner
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(worker);
        self.inner.started.store(true, Ordering::Release);
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.shutdown_worker();
        Ok(())
    }

    fn health(&self) -> Health {
        let (status, detail) = self.inner.health_parts();
        Health { status, detail }
    }

    fn readiness(&self) -> Readiness {
        match self.health().status {
            HealthStatus::Healthy => Readiness::Ready,
            HealthStatus::PermissionRequired => Readiness::PermissionRequired,
            HealthStatus::Degraded
            | HealthStatus::Unhealthy
            | HealthStatus::Unknown
            | HealthStatus::Unsupported => Readiness::Unknown,
        }
    }

    fn capabilities(&self) -> &'static [CapabilityRequirement] {
        REQUIREMENTS
    }

    fn configuration(&self) -> ServiceConfiguration {
        ServiceConfiguration {
            id: ServiceId::Screenshot,
            version: self.inner.version,
            monitoring_enabled: false,
        }
    }

    fn capture_screenshot(&mut self) -> Result<CaptureReport> {
        let started = Instant::now();
        match self.capture_once() {
            Ok(stored) => Ok(CaptureReport {
                width: stored.width,
                height: stored.height,
                byte_len: stored.byte_len,
                duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                frames: 1,
                artifact: stored.path.display().to_string(),
                captured_at: stored.captured_at,
            }),
            Err(error) => Err(Error::new(
                ServiceId::Screenshot.as_str(),
                "capture_screenshot",
                error.to_string(),
            )
            .with_service(ServiceId::Screenshot.as_str())),
        }
    }

    fn capture_interval_screenshot(&mut self) -> Result<CaptureReport> {
        let started = Instant::now();
        match self.capture_for_interval() {
            Ok(stored) => Ok(CaptureReport {
                width: stored.width,
                height: stored.height,
                byte_len: stored.byte_len,
                duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                frames: 1,
                artifact: stored.path.display().to_string(),
                captured_at: stored.captured_at,
            }),
            Err(error) => Err(Error::new(
                ServiceId::Screenshot.as_str(),
                "capture_screenshot",
                error.to_string(),
            )
            .with_service(ServiceId::Screenshot.as_str())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ScreenshotService;
    use crate::contract::{
        FakeOutcome, FakeScreenshotProvider, Screenshot, ScreenshotError, ScreenshotProvider,
        ScreenshotRequest, ONE_PIXEL_PNG,
    };
    use crate::create;
    use std::fs;
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;
    use wind_core::{CapabilityStatus, HealthStatus, Service};

    fn directory(label: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "wind-screenshot-service-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        directory
    }

    fn started(label: &str, outcome: FakeOutcome) -> (ScreenshotService, std::path::PathBuf) {
        let directory = directory(label);
        let mut service = create(
            directory.clone(),
            Box::new(FakeScreenshotProvider::new(outcome)),
        )
        .expect("service");
        service.start().expect("start");
        (service, directory)
    }

    #[test]
    fn missing_permission_does_not_fail_startup_or_capture_a_file() {
        let (service, directory) = started("permission", FakeOutcome::PermissionRequired);
        assert_eq!(service.health().status, HealthStatus::PermissionRequired);
        assert_ne!(service.health().status, HealthStatus::Unhealthy);
        assert!(!service.configuration().monitoring_enabled);
        assert!(!directory.exists());
        let error = service.capture_once().expect_err("permission");
        assert!(matches!(error, ScreenshotError::PermissionRequired { .. }));
        assert_eq!(service.health().status, HealthStatus::PermissionRequired);
        assert!(!directory.exists());
    }

    #[test]
    fn unsupported_capture_stays_running() {
        let (service, directory) = started("unsupported", FakeOutcome::Unsupported);
        assert_eq!(service.health().status, HealthStatus::Unsupported);
        let error = service.capture_once().expect_err("unsupported");
        assert!(matches!(error, ScreenshotError::Unsupported { .. }));
        assert!(!directory.exists());
    }

    #[test]
    fn capture_failure_is_unhealthy_without_a_startup_failure() {
        let (mut service, directory) = started("capture", FakeOutcome::CaptureFailed);
        let error = service.capture_once().expect_err("capture");
        assert!(matches!(error, ScreenshotError::CaptureFailed { .. }));
        assert_eq!(service.health().status, HealthStatus::Unhealthy);
        service.stop().expect("stop");
        assert_eq!(service.health().status, HealthStatus::Unknown);
        assert!(!directory.exists());
    }

    #[test]
    fn storage_failure_degrades_the_service() {
        let blocked = directory("storage");
        fs::write(&blocked, b"not a directory").expect("file");
        let mut service = create(
            blocked.clone(),
            Box::new(FakeScreenshotProvider::new(FakeOutcome::Success)),
        )
        .expect("service");
        service.start().expect("start");
        let error = service.capture_once().expect_err("storage");
        assert!(matches!(error, ScreenshotError::StorageFailed { .. }));
        assert_eq!(service.health().status, HealthStatus::Degraded);
        service.stop().expect("stop");
        let _ = fs::remove_file(&blocked);
    }

    #[test]
    fn granted_capture_writes_one_png_and_a_second_call_can_repeat() {
        let (service, directory) = started("success", FakeOutcome::Success);
        assert_eq!(service.health().status, HealthStatus::Healthy);
        let first = service.capture_once().expect("first");
        let second = service.capture_once().expect("second");
        assert_ne!(first.path, second.path);
        assert!(first.path.starts_with(&directory));
        assert_eq!(fs::read(&first.path).expect("png")[..8], ONE_PIXEL_PNG[..8]);
        assert_eq!(service.health().status, HealthStatus::Healthy);
        assert!(!format!("{first:?}").contains("png"));
        let _ = fs::remove_dir_all(&directory);
    }

    struct Gate {
        entered: Mutex<bool>,
        entered_wait: Condvar,
        release: Mutex<bool>,
        release_wait: Condvar,
    }

    impl ScreenshotProvider for Gate {
        fn capture(&self, _request: &ScreenshotRequest) -> Result<Screenshot, ScreenshotError> {
            {
                let mut entered = self
                    .entered
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *entered = true;
                self.entered_wait.notify_one();
            }
            let mut release = self
                .release
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            while !*release {
                let (guard, timeout) = self
                    .release_wait
                    .wait_timeout(release, Duration::from_secs(2))
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                release = guard;
                if timeout.timed_out() {
                    return Err(ScreenshotError::CaptureFailed {
                        reason: "test gate timed out".to_string(),
                    });
                }
            }
            Ok(crate::contract::sample_screenshot())
        }

        fn capability_status(&self) -> CapabilityStatus {
            CapabilityStatus::Available
        }
    }

    #[test]
    fn a_second_capture_is_rejected_while_one_is_active() {
        let directory = directory("gate");
        let gate = Arc::new(Gate {
            entered: Mutex::new(false),
            entered_wait: Condvar::new(),
            release: Mutex::new(false),
            release_wait: Condvar::new(),
        });
        let service = Arc::new({
            let mut service = create(directory.clone(), Box::new(GateProvider(Arc::clone(&gate))))
                .expect("service");
            service.start().expect("start");
            service
        });
        let worker = Arc::clone(&service);
        let running = std::thread::spawn(move || worker.capture_once());
        {
            let guard = gate
                .entered
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (guard, timeout) = gate
                .entered_wait
                .wait_timeout_while(guard, Duration::from_secs(2), |entered| !*entered)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert!(!timeout.timed_out(), "first capture did not start");
            drop(guard);
        }
        let rejected = service.capture_once().expect_err("busy");
        assert!(matches!(rejected, ScreenshotError::AlreadyInProgress));
        *gate
            .release
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        gate.release_wait.notify_one();
        let stored = running.join().expect("worker").expect("capture");
        assert!(stored.path.starts_with(&directory));
        let _ = fs::remove_dir_all(&directory);
    }

    struct GateProvider(Arc<Gate>);

    impl ScreenshotProvider for GateProvider {
        fn capture(&self, request: &ScreenshotRequest) -> Result<Screenshot, ScreenshotError> {
            self.0.capture(request)
        }

        fn capability_status(&self) -> CapabilityStatus {
            self.0.capability_status()
        }
    }
}
