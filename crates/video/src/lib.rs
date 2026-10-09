//! Video capability.
//!
//! The service stores one explicit recording at a time. The operating-system
//! capture and encoder live behind [`VideoProvider`].

mod contract;
mod queue;
mod service;
mod storage;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use wind_core::{Error, Result, ServiceId, Version};
use wind_storage::LocalStore;

pub use contract::{
    mp4_duration, DisplaySelection, FakeOutcome, FakeVideoProvider, VideoConfig, VideoError,
    VideoProvider, VideoResult, FINALIZED_MARKER,
};
pub use queue::{DropQueue, PushResult, FRAME_QUEUE_CAPACITY};
pub use service::VideoService;

/// `<data_dir>/videos`. The directory is not created until a recording starts.
pub fn video_directory(data_dir: &Path) -> PathBuf {
    data_dir.join("videos")
}

/// Build the video service around `provider`.
///
/// `storage_dir` should be [`video_directory`] applied to the runtime data
/// directory. The provider performs the recording. This function does not.
pub fn create(storage_dir: PathBuf, provider: Box<dyn VideoProvider>) -> Result<VideoService> {
    create_with_store(storage_dir, provider, None)
}

pub fn create_with_store(
    storage_dir: PathBuf,
    provider: Box<dyn VideoProvider>,
    catalog: Option<Arc<dyn LocalStore>>,
) -> Result<VideoService> {
    let version = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|error| {
        Error::new(ServiceId::Video.as_str(), "read_version", error.to_string())
            .with_service(ServiceId::Video.as_str())
    })?;
    Ok(VideoService::new(
        version,
        storage::VideoStorage::new(storage_dir),
        provider,
        catalog,
    ))
}
