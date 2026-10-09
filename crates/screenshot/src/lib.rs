//! Screenshot capability.
//!
//! The service stores one explicit capture at a time. The operating-system
//! capture implementation lives behind [`ScreenshotProvider`].

mod contract;
mod service;
mod storage;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use wind_core::{Error, Result, ServiceId, Version};

pub use contract::{
    FakeOutcome, FakeScreenshotProvider, ImageFormat, Screenshot, ScreenshotError,
    ScreenshotProvider, ScreenshotRequest,
};
pub use service::ScreenshotService;
pub use storage::StoredScreenshot;

/// `<data_dir>/screenshots`. The directory is not created until a capture is stored.
pub fn screenshot_directory(data_dir: &Path) -> PathBuf {
    data_dir.join("screenshots")
}

/// Build the screenshot service around `provider`.
///
/// `storage_dir` should be [`screenshot_directory`] applied to the runtime data
/// directory. The provider performs the capture. This function does not.
pub fn create(
    storage_dir: PathBuf,
    provider: Box<dyn ScreenshotProvider>,
) -> Result<ScreenshotService> {
    create_with_store(storage_dir, provider, None)
}

pub fn create_with_store(
    storage_dir: PathBuf,
    provider: Box<dyn ScreenshotProvider>,
    catalog: Option<Arc<dyn wind_storage::LocalStore>>,
) -> Result<ScreenshotService> {
    let version = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|error| {
        Error::new(
            ServiceId::Screenshot.as_str(),
            "read_version",
            error.to_string(),
        )
        .with_service(ServiceId::Screenshot.as_str())
    })?;
    Ok(ScreenshotService::new(
        version,
        storage::ScreenshotStorage::new(storage_dir),
        Arc::from(provider),
        catalog,
    ))
}
