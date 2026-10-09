//! Local durable storage.
//!
//! Structured tracking records go in one SQLite database. Screenshot and video
//! bytes stay on the filesystem. Secrets stay in the operating-system store.
//! This crate does not contact a network.

mod secret;
mod sqlite;

use std::path::PathBuf;
use std::time::SystemTime;

use wind_core::{emit, Level, LogFields};

pub use secret::{check_name, MemorySecretStore, SecretError, SecretStore};
pub use sqlite::{SqliteStore, StoreOptions, DEFAULT_MAX_LOCAL_STORAGE_BYTES};

/// Accept or rewrite a URL before it is stored.
///
/// The supervisor installs the existing URL sanitizer. A failure must not
/// include the URL text.
pub type UrlSanitizer = fn(&str) -> Result<String, String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactKind {
    Screenshot,
    Video,
}

impl ArtifactKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Screenshot => "screenshot",
            Self::Video => "video",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncStatus {
    Pending,
    Uploading,
    Uploaded,
    Failed,
}

impl SyncStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Uploading => "uploading",
            Self::Uploaded => "uploaded",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "uploading" => Some(Self::Uploading),
            "uploaded" => Some(Self::Uploaded),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityCounts {
    pub keyboard_events: u64,
    pub mouse_moves: u64,
    pub mouse_clicks: u64,
    pub mouse_scrolls: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationSegment {
    pub application_id: String,
    pub started_at: SystemTime,
    pub ended_at: SystemTime,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UrlSegment {
    pub url: String,
    pub started_at: SystemTime,
    pub ended_at: SystemTime,
}

#[derive(Clone, Debug, Default)]
pub struct IntervalParts {
    pub activity: Option<ActivityCounts>,
    pub application: Option<ApplicationSegment>,
    pub url: Option<UrlSegment>,
}

#[derive(Clone, Debug)]
pub struct IntervalDraft {
    pub started_at: SystemTime,
    pub ended_at: SystemTime,
    pub activity: Option<ActivityCounts>,
    pub application: Option<ApplicationSegment>,
    pub url: Option<UrlSegment>,
}

#[derive(Clone, Debug)]
pub struct ArtifactRequest {
    pub kind: ArtifactKind,
    pub path: PathBuf,
    pub captured_at: SystemTime,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingRecord {
    pub sequence_number: u64,
    pub record_type: String,
    pub record_id: String,
    pub previous_hash: String,
    pub record_hash: String,
    pub sync_status: SyncStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageError {
    Unavailable { reason: String },
    Integrity { reason: String },
    Exhausted { reason: String },
    Rejected { reason: String },
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable { reason } => {
                write!(formatter, "local storage is unavailable: {reason}")
            }
            Self::Integrity { reason } => {
                write!(formatter, "local storage integrity check failed: {reason}")
            }
            Self::Exhausted { reason } => write!(formatter, "local storage is exhausted: {reason}"),
            Self::Rejected { reason } => {
                write!(formatter, "local storage rejected the record: {reason}")
            }
        }
    }
}

impl std::error::Error for StorageError {}

impl StorageError {
    fn unavailable(reason: impl Into<String>) -> Self {
        Self::Unavailable {
            reason: reason.into(),
        }
    }

    fn integrity(reason: impl Into<String>) -> Self {
        Self::Integrity {
            reason: reason.into(),
        }
    }

    fn exhausted(reason: impl Into<String>) -> Self {
        Self::Exhausted {
            reason: reason.into(),
        }
    }

    fn rejected(reason: impl Into<String>) -> Self {
        Self::Rejected {
            reason: reason.into(),
        }
    }
}

/// Business storage operations. Implementations do not expose SQL.
pub trait LocalStore: Send + Sync {
    fn create_interval(&self, started_at: SystemTime) -> Result<String, StorageError>;
    fn finish_interval(
        &self,
        id: &str,
        ended_at: SystemTime,
        parts: IntervalParts,
    ) -> Result<(), StorageError>;
    fn record_interval(&self, draft: IntervalDraft) -> Result<String, StorageError>;
    fn register_artifact(&self, request: ArtifactRequest) -> Result<String, StorageError>;
    /// Attach a finished file to an interval that already exists.
    fn attach_artifact(
        &self,
        interval_id: &str,
        request: ArtifactRequest,
    ) -> Result<String, StorageError>;
    fn pending_records(&self) -> Result<Vec<PendingRecord>, StorageError>;
    fn acknowledge(&self, sequence_number: u64) -> Result<(), StorageError>;
    fn validate_chain(&self) -> Result<(), StorageError>;
}

pub fn report_secret_unavailable() {
    log_storage(
        Level::Error,
        "secret_store_unavailable",
        Some("operating system secret store is unavailable"),
    );
}

pub(crate) fn log_storage(level: Level, message: &str, error: Option<&str>) {
    emit(&LogFields {
        level,
        component: "storage",
        service_id: None,
        version: None,
        platform: None,
        message,
        error,
        state: Some("Running"),
    });
}
