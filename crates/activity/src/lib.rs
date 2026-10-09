//! Aggregate keyboard and mouse activity.
//!
//! The service reports how much interaction happened. It does not report what
//! was typed or where the pointer was. Platform listeners reduce each
//! operating-system event to a counter before the service can see it.

mod contract;
mod service;

pub use contract::{
    ActivityCounters, ActivityError, ActivityProvider, ActivitySnapshot, FakeActivityOutcome,
    FakeActivityProvider,
};
pub use service::ActivityService;

use std::sync::Arc;

use wind_core::Result;
use wind_storage::LocalStore;

pub fn create(provider: Box<dyn ActivityProvider>) -> Result<ActivityService> {
    create_with_store(provider, None)
}

pub fn create_with_store(
    provider: Box<dyn ActivityProvider>,
    store: Option<Arc<dyn LocalStore>>,
) -> Result<ActivityService> {
    let version = wind_core::Version::parse(env!("CARGO_PKG_VERSION")).map_err(|error| {
        wind_core::Error::new("activity", "read_version", error.to_string())
            .with_service("activity")
    })?;
    Ok(ActivityService::with_store(version, provider, store))
}
