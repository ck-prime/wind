//! Video service.
//!
//! Recording runs on one worker and only when `start_recording` is called.
//! The service lifecycle stays `Running` while a recording is in progress.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::SystemTime;

use wind_core::{
    emit, Capability, CapabilityRequirement, CapabilityStatus, CaptureReport, Error, ExecutionMode,
    Health, HealthStatus, Level, LogFields, Readiness, Result, Service, ServiceConfiguration,
    ServiceId, Version,
};

use wind_storage::{ArtifactKind, ArtifactRequest, LocalStore};

use crate::contract::{VideoConfig, VideoError, VideoProvider, VideoResult};
use crate::storage::VideoStorage;

const REQUIREMENTS: &[CapabilityRequirement] = &[CapabilityRequirement {
    capability: Capability::VideoCapture,
    required: true,
}];

enum WorkerRequest {
    Start {
        reply: Sender<std::result::Result<(), VideoError>>,
    },
    Stop {
        reply: Sender<std::result::Result<StoredVideo, VideoError>>,
    },
    Shutdown,
}

struct StoredVideo {
    result: VideoResult,
    path: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HealthNote {
    Clear,
    Degraded,
    Unhealthy,
}

struct Inner {
    provider: Mutex<Box<dyn VideoProvider>>,
    storage: VideoStorage,
    version: Version,
    started: AtomicBool,
    stopping: AtomicBool,
    recording: AtomicBool,
    note: Mutex<HealthNote>,
    sender: Mutex<Option<mpsc::Sender<WorkerRequest>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    active_path: Mutex<Option<PathBuf>>,
    catalog: Option<Arc<dyn LocalStore>>,
}

/// In-process video service.
///
/// `monitoring_enabled` stays false. Nothing is recorded until
/// [`VideoService::start_recording`].
pub struct VideoService {
    inner: Arc<Inner>,
}

impl VideoService {
    pub(crate) fn new(
        version: Version,
        storage: VideoStorage,
        provider: Box<dyn VideoProvider>,
        catalog: Option<Arc<dyn LocalStore>>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                provider: Mutex::new(provider),
                storage,
                version,
                started: AtomicBool::new(false),
                stopping: AtomicBool::new(false),
                recording: AtomicBool::new(false),
                note: Mutex::new(HealthNote::Clear),
                sender: Mutex::new(None),
                worker: Mutex::new(None),
                active_path: Mutex::new(None),
                catalog,
            }),
        }
    }

    /// Start recording the primary display. The file is not finalized yet.
    pub fn start_recording(&self) -> std::result::Result<(), VideoError> {
        self.send_start()
    }

    /// Stop recording, finalize the file, and return its metadata.
    pub fn stop_recording(&self) -> std::result::Result<CaptureReport, VideoError> {
        let stored = self.send_stop()?;
        Ok(report(stored))
    }

    fn send_start(&self) -> std::result::Result<(), VideoError> {
        if !self.inner.started.load(Ordering::Acquire)
            || self.inner.stopping.load(Ordering::Acquire)
        {
            return Err(VideoError::Stopped);
        }
        let (reply_tx, reply_rx) = mpsc::channel();
        let sent = self
            .inner
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|sender| sender.send(WorkerRequest::Start { reply: reply_tx }));
        if !matches!(sent, Some(Ok(()))) {
            return Err(VideoError::Stopped);
        }
        reply_rx.recv().unwrap_or(Err(VideoError::Stopped))
    }

    fn send_stop(&self) -> std::result::Result<StoredVideo, VideoError> {
        if !self.inner.started.load(Ordering::Acquire) {
            return Err(VideoError::Stopped);
        }
        let (reply_tx, reply_rx) = mpsc::channel();
        let sent = self
            .inner
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|sender| sender.send(WorkerRequest::Stop { reply: reply_tx }));
        if !matches!(sent, Some(Ok(()))) {
            return Err(VideoError::Stopped);
        }
        reply_rx.recv().unwrap_or(Err(VideoError::Stopped))
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
        self.inner.recording.store(false, Ordering::Release);
    }
}

impl Drop for VideoService {
    fn drop(&mut self) {
        self.shutdown_worker();
    }
}

impl Inner {
    fn run(self: &Arc<Self>, receiver: Receiver<WorkerRequest>) {
        while let Ok(request) = receiver.recv() {
            match request {
                WorkerRequest::Shutdown => {
                    if self.recording.load(Ordering::Acquire) {
                        let _ = self.finish();
                    }
                    break;
                }
                WorkerRequest::Start { reply } => {
                    let result = if self.stopping.load(Ordering::Acquire) {
                        Err(VideoError::Stopped)
                    } else {
                        catch_unwind(AssertUnwindSafe(|| self.begin())).unwrap_or_else(|_| {
                            Err(VideoError::CaptureFailed {
                                reason: "video worker panicked".to_string(),
                            })
                        })
                    };
                    self.record_start(&result);
                    let _ = reply.send(result);
                }
                WorkerRequest::Stop { reply } => {
                    let result =
                        catch_unwind(AssertUnwindSafe(|| self.finish())).unwrap_or_else(|_| {
                            Err(VideoError::EncodingFailed {
                                reason: "video worker panicked".to_string(),
                            })
                        });
                    self.record_stop(&result);
                    let _ = reply.send(result);
                }
            }
        }
    }

    fn begin(&self) -> std::result::Result<(), VideoError> {
        if self.recording.load(Ordering::Acquire) {
            return Err(VideoError::AlreadyRecording);
        }
        let status = self
            .provider
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .capability_status();
        if status != CapabilityStatus::Available {
            return Err(status_error(status));
        }
        let config = VideoConfig::default().validated()?;
        let path = self.storage.reserve()?;
        let started = self
            .provider
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .start(&path, &config);
        if let Err(error) = started {
            self.storage.discard(&path);
            return Err(error);
        }
        log_event(self.version, "video_recording_started");
        *self
            .active_path
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(path);
        self.recording.store(true, Ordering::Release);
        Ok(())
    }

    fn finish(&self) -> std::result::Result<StoredVideo, VideoError> {
        if !self.recording.load(Ordering::Acquire) {
            return Err(VideoError::NotRecording);
        }
        let path = self
            .active_path
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let result = self
            .provider
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .stop();
        self.recording.store(false, Ordering::Release);
        match (path, result) {
            (Some(path), Ok(result)) => {
                let byte_len = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                if byte_len == 0 {
                    self.storage.discard(&path);
                    return Err(VideoError::EncodingFailed {
                        reason: "finalized video is empty".to_string(),
                    });
                }
                if let Some(catalog) = &self.catalog {
                    if catalog
                        .register_artifact(ArtifactRequest {
                            kind: ArtifactKind::Video,
                            path: path.clone(),
                            captured_at: SystemTime::now(),
                        })
                        .is_err()
                    {
                        return Err(VideoError::StorageFailed {
                            reason: "video metadata could not be saved".to_string(),
                        });
                    }
                }
                log_stopped(self.version, &result, byte_len);
                Ok(StoredVideo { result, path })
            }
            (Some(path), Err(error)) => {
                self.storage.discard(&path);
                Err(error)
            }
            (None, _) => Err(VideoError::StorageFailed {
                reason: "recording has no output path".to_string(),
            }),
        }
    }

    fn record_start(&self, result: &std::result::Result<(), VideoError>) {
        if let Err(error) = result {
            self.note_for(error);
            log_failed(self.version, error);
        }
    }

    fn record_stop(&self, result: &std::result::Result<StoredVideo, VideoError>) {
        match result {
            Ok(_) => {
                *self
                    .note
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = HealthNote::Clear;
            }
            Err(error) => {
                self.note_for(error);
                log_failed(self.version, error);
            }
        }
    }

    fn note_for(&self, error: &VideoError) {
        let note = match error {
            VideoError::CaptureFailed { .. } => HealthNote::Unhealthy,
            VideoError::EncodingFailed { .. }
            | VideoError::StorageFailed { .. }
            | VideoError::ProviderUnavailable { .. }
            | VideoError::NoDisplayAvailable { .. } => HealthNote::Degraded,
            VideoError::PermissionRequired { .. }
            | VideoError::Unsupported { .. }
            | VideoError::AlreadyRecording
            | VideoError::NotRecording
            | VideoError::Stopped => HealthNote::Clear,
        };
        *self
            .note
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = note;
    }

    fn health_parts(&self) -> (HealthStatus, &'static str) {
        if !self.started.load(Ordering::Acquire) {
            return (HealthStatus::Unknown, "video service is not running");
        }
        let status = self
            .provider
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .capability_status();
        match status {
            CapabilityStatus::Unsupported => (
                HealthStatus::Unsupported,
                "video capture is not implemented on this operating system",
            ),
            CapabilityStatus::PermissionRequired => (
                HealthStatus::PermissionRequired,
                "video capture permission is required",
            ),
            CapabilityStatus::Unknown => (HealthStatus::Unknown, "video capture status is unknown"),
            CapabilityStatus::Unavailable => (
                HealthStatus::Degraded,
                "video capture is temporarily unavailable",
            ),
            CapabilityStatus::Available => {
                match *self
                    .note
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                {
                    HealthNote::Unhealthy => (HealthStatus::Unhealthy, "video capture failed"),
                    HealthNote::Degraded => {
                        (HealthStatus::Degraded, "video encoding or storage failed")
                    }
                    HealthNote::Clear => (
                        HealthStatus::Healthy,
                        "video capture is available; recording starts only when requested",
                    ),
                }
            }
        }
    }
}

fn report(stored: StoredVideo) -> CaptureReport {
    CaptureReport {
        width: stored.result.width,
        height: stored.result.height,
        byte_len: stored.result.byte_len,
        duration_ms: u64::try_from(stored.result.duration.as_millis()).unwrap_or(u64::MAX),
        frames: stored.result.frame_count,
        artifact: stored.path.display().to_string(),
        captured_at: SystemTime::now(),
    }
}

fn status_error(status: CapabilityStatus) -> VideoError {
    match status {
        CapabilityStatus::PermissionRequired => VideoError::PermissionRequired {
            state: wind_core::PermissionState::NotDetermined,
            reason: "video capture permission is required".to_string(),
        },
        CapabilityStatus::Unsupported => VideoError::Unsupported {
            reason: "video capture is not implemented on this operating system".to_string(),
        },
        CapabilityStatus::Unavailable => VideoError::ProviderUnavailable {
            reason: "video capture is temporarily unavailable".to_string(),
        },
        CapabilityStatus::Unknown => VideoError::ProviderUnavailable {
            reason: "video capture status is unknown".to_string(),
        },
        CapabilityStatus::Available => VideoError::CaptureFailed {
            reason: "video capture status changed during recording".to_string(),
        },
    }
}

fn log_event(version: Version, message: &str) {
    let version = version.to_string();
    emit(&LogFields {
        level: Level::Info,
        component: "video",
        service_id: Some(ServiceId::Video.as_str()),
        version: Some(&version),
        platform: None,
        message,
        error: None,
        state: Some("Running"),
    });
}

fn log_stopped(version: Version, result: &VideoResult, byte_len: u64) {
    let message = format!(
        "video_recording_stopped duration_ms={} frames={} dropped={} bytes={byte_len}",
        result.duration.as_millis(),
        result.frame_count,
        result.dropped_frames
    );
    log_event(version, &message);
}

fn log_failed(version: Version, error: &VideoError) {
    let version = version.to_string();
    let text = error.to_string();
    emit(&LogFields {
        level: Level::Error,
        component: "video",
        service_id: Some(ServiceId::Video.as_str()),
        version: Some(&version),
        platform: None,
        message: "video_recording_failed",
        error: Some(&text),
        state: Some("Running"),
    });
}

impl Service for VideoService {
    fn id(&self) -> ServiceId {
        ServiceId::Video
    }

    fn name(&self) -> &str {
        "video"
    }

    fn version(&self) -> Version {
        self.inner.version
    }

    fn execution_mode(&self) -> ExecutionMode {
        ExecutionMode::InProcess
    }

    fn start(&mut self) -> Result<()> {
        if self.inner.started.load(Ordering::Acquire) {
            return Ok(());
        }
        let (sender, receiver) = mpsc::channel();
        let worker_inner = Arc::clone(&self.inner);
        let worker = thread::Builder::new()
            .name("wind-video".to_string())
            .spawn(move || worker_inner.run(receiver))
            .map_err(|error| {
                Error::new(
                    ServiceId::Video.as_str(),
                    "start",
                    format!("video worker could not start: {error}"),
                )
                .with_service(ServiceId::Video.as_str())
            })?;
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
        self.inner.stopping.store(false, Ordering::Release);
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
        if self.inner.started.load(Ordering::Acquire) {
            Readiness::Ready
        } else {
            Readiness::Unknown
        }
    }

    fn capabilities(&self) -> &'static [CapabilityRequirement] {
        REQUIREMENTS
    }

    fn configuration(&self) -> ServiceConfiguration {
        ServiceConfiguration {
            id: ServiceId::Video,
            version: self.inner.version,
            monitoring_enabled: false,
        }
    }

    fn start_recording(&mut self) -> Result<()> {
        self.send_start().map_err(|error| {
            Error::new(
                ServiceId::Video.as_str(),
                "start_recording",
                error.to_string(),
            )
            .with_service(ServiceId::Video.as_str())
        })
    }

    fn stop_recording(&mut self) -> Result<CaptureReport> {
        self.send_stop().map(report).map_err(|error| {
            Error::new(
                ServiceId::Video.as_str(),
                "stop_recording",
                error.to_string(),
            )
            .with_service(ServiceId::Video.as_str())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::VideoService;
    use crate::contract::{FakeOutcome, FakeVideoProvider, FINALIZED_MARKER};
    use crate::create;
    use std::fs;
    use wind_core::{HealthStatus, Service};

    fn directory(label: &str) -> std::path::PathBuf {
        let directory =
            std::env::temp_dir().join(format!("wind-video-service-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        directory
    }

    fn started(label: &str, outcome: FakeOutcome) -> (VideoService, std::path::PathBuf) {
        let directory = directory(label);
        let mut service =
            create(directory.clone(), Box::new(FakeVideoProvider::new(outcome))).expect("create");
        service.start().expect("start");
        (service, directory)
    }

    #[test]
    fn startup_does_not_create_a_video() {
        let (service, directory) = started("idle", FakeOutcome::Success);
        assert_eq!(service.health().status, HealthStatus::Healthy);
        assert!(!directory.exists());
        assert!(!service.configuration().monitoring_enabled);
    }

    #[test]
    fn start_and_stop_write_one_finalized_file() {
        let (service, directory) = started("once", FakeOutcome::Success);
        service.start_recording().expect("start recording");
        let report = service.stop_recording().expect("stop");
        let bytes = fs::read(&report.artifact).expect("read");
        assert_eq!(bytes, FINALIZED_MARKER);
        assert!(report.byte_len > 0);
        assert!(directory
            .join(std::path::Path::new(&report.artifact).file_name().unwrap())
            .exists());
    }

    #[test]
    fn recording_can_be_repeated() {
        let (service, _) = started("repeat", FakeOutcome::Success);
        let first = {
            service.start_recording().expect("first start");
            service.stop_recording().expect("first stop").artifact
        };
        let second = {
            service.start_recording().expect("second start");
            service.stop_recording().expect("second stop").artifact
        };
        assert_ne!(first, second);
        assert_eq!(fs::read(&first).expect("first"), FINALIZED_MARKER);
        assert_eq!(fs::read(&second).expect("second"), FINALIZED_MARKER);
    }

    #[test]
    fn a_second_start_is_rejected() {
        let (service, _) = started("busy", FakeOutcome::Success);
        service.start_recording().expect("start");
        let error = service.start_recording().expect_err("second");
        let text = error.to_string();
        assert!(text.contains("already"), "{text}");
        service.stop_recording().expect("stop");
    }

    #[test]
    fn permission_required_does_not_write_a_file() {
        let (service, directory) = started("permission", FakeOutcome::PermissionRequired);
        assert_eq!(service.health().status, HealthStatus::PermissionRequired);
        let error = service.start_recording().expect_err("permission");
        assert!(error.to_string().contains("permission"), "{error}");
        assert!(!directory.exists());
        assert_eq!(service.health().status, HealthStatus::PermissionRequired);
    }

    #[test]
    fn unsupported_stays_running() {
        let (mut service, _) = started("unsupported", FakeOutcome::Unsupported);
        assert_eq!(service.health().status, HealthStatus::Unsupported);
        service.stop().expect("stop");
        assert_eq!(service.health().status, HealthStatus::Unknown);
    }

    #[test]
    fn capture_failure_is_unhealthy_and_recording_can_be_retried() {
        let directory = directory("capture");
        let mut service = create(
            directory.clone(),
            Box::new(FakeVideoProvider::new(FakeOutcome::CaptureFailed)),
        )
        .expect("create");
        service.start().expect("start");
        let error = service.start_recording().expect_err("capture");
        assert!(error.to_string().contains("failed"), "{error}");
        assert_eq!(service.health().status, HealthStatus::Unhealthy);
        assert!(!directory.exists() || fs::read_dir(&directory).unwrap().next().is_none());
    }

    #[test]
    fn encoder_failure_removes_the_partial_file() {
        let (service, directory) = started("encode", FakeOutcome::EncodingFailed);
        service.start_recording().expect("start");
        let error = service.stop_recording().expect_err("encode");
        assert!(error.to_string().contains("encoder"), "{error}");
        assert_eq!(service.health().status, HealthStatus::Degraded);
        let entries = fs::read_dir(&directory)
            .map(|iter| iter.count())
            .unwrap_or(0);
        assert_eq!(entries, 0);
    }

    #[test]
    fn shutdown_finalizes_an_active_recording() {
        let (mut service, _) = started("shutdown", FakeOutcome::Success);
        service.start_recording().expect("start");
        service.stop().expect("stop");
        assert_eq!(service.health().status, HealthStatus::Unknown);
    }

    #[test]
    fn a_finalized_video_registers_metadata_without_storing_bytes() {
        let data = std::env::temp_dir().join(format!("wind-video-meta-{}", std::process::id()));
        let _ = fs::remove_dir_all(&data);
        let store = wind_storage::SqliteStore::open(
            &data,
            wind_storage::StoreOptions {
                max_local_storage_bytes: 1024 * 1024,
                config_version: "0.1.0".to_string(),
                user_id: None,
                sanitize_url: |_| Err("url was rejected".to_string()),
            },
        )
        .expect("open");
        let database = store.database_path().to_path_buf();
        let catalog: std::sync::Arc<dyn wind_storage::LocalStore> = std::sync::Arc::new(store);
        let videos = data.join("videos");
        let mut service = crate::create_with_store(
            videos,
            Box::new(FakeVideoProvider::new(FakeOutcome::Success)),
            Some(catalog),
        )
        .expect("create");
        service.start().expect("start");
        service.start_recording().expect("record");
        service.stop_recording().expect("finish");
        drop(service);
        let mut bytes = fs::read(&database).unwrap_or_default();
        if let Ok(wal) = fs::read(database.with_extension("db-wal")) {
            bytes.extend(wal);
        }
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("video"));
        assert!(text.contains("pending"));
        assert!(!bytes
            .windows(FINALIZED_MARKER.len())
            .any(|window| window == FINALIZED_MARKER));
        use wind_storage::LocalStore;
        let reopened = wind_storage::SqliteStore::open(
            &data,
            wind_storage::StoreOptions {
                max_local_storage_bytes: 1024 * 1024,
                config_version: "0.1.0".to_string(),
                user_id: None,
                sanitize_url: |_| Err("url was rejected".to_string()),
            },
        )
        .expect("reopen");
        let pending = reopened.pending_records().expect("pending");
        assert!(pending
            .iter()
            .any(|record| record.record_type == "artifact"));
        drop(reopened);
        let _ = fs::remove_dir_all(data);
    }
}
