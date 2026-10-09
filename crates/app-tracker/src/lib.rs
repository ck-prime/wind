//! Foreground application tracker.
//!
//! The service asks a platform provider for the current application. It does
//! not read window titles, process memory, command lines, or environment
//! variables, and it does not poll on its own.

mod contract;
mod service;

use std::sync::Arc;

use wind_core::{Error, Result, ServiceId, Version};
use wind_storage::LocalStore;

pub use contract::{
    ActiveApplication, AppError, AppTrackerProvider, FakeAppOutcome, FakeAppTracker,
};
pub use service::AppTrackerService;

/// Build the application tracker around `provider`.
///
/// Starting the service does not query the foreground application.
pub fn create(provider: Box<dyn AppTrackerProvider>) -> Result<AppTrackerService> {
    create_with_store(provider, None)
}

pub fn create_with_store(
    provider: Box<dyn AppTrackerProvider>,
    store: Option<Arc<dyn LocalStore>>,
) -> Result<AppTrackerService> {
    let version = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|error| {
        Error::new(
            ServiceId::AppTracker.as_str(),
            "read_version",
            error.to_string(),
        )
        .with_service(ServiceId::AppTracker.as_str())
    })?;
    Ok(AppTrackerService::with_store(version, provider, store))
}
