//! Browser URL tracker.
//!
//! The current URL is read from the foreground browser's accessibility tree
//! when `current_url` is called. The service does not read window titles,
//! browser memory, cookies, page content, or a browser history database, and
//! it does not poll.

mod browser;
mod contract;
mod sanitize;
mod service;

use std::sync::Arc;

use wind_core::{Error, Result, ServiceId, Version};
use wind_storage::LocalStore;

pub use browser::{classify_browser_bundle, classify_browser_executable};
pub use contract::{
    BrowserIntegration, BrowserKind, FakeUrlOutcome, FakeUrlTracker, UrlError, UrlObservation,
    UrlTrackerProvider,
};
pub use sanitize::{sanitize_address_bar, sanitize_http_url};
pub use service::UrlTrackerService;

/// Build the URL tracker around `provider`.
///
/// Starting the service does not invent a URL.
pub fn create(provider: Box<dyn UrlTrackerProvider>) -> Result<UrlTrackerService> {
    create_with_store(provider, None)
}

pub fn create_with_store(
    provider: Box<dyn UrlTrackerProvider>,
    store: Option<Arc<dyn LocalStore>>,
) -> Result<UrlTrackerService> {
    let version = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|error| {
        Error::new(
            ServiceId::UrlTracker.as_str(),
            "read_version",
            error.to_string(),
        )
        .with_service(ServiceId::UrlTracker.as_str())
    })?;
    Ok(UrlTrackerService::with_store(version, provider, store))
}
