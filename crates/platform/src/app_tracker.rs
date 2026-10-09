//! Host foreground-application provider.
//!
//! The service sees [`AppTrackerProvider`]. Native window and process types
//! stay in the platform crate for this operating system.

use wind_app_tracker::{ActiveApplication, AppError, AppTrackerProvider};
use wind_core::CapabilityStatus;

/// Foreground application provider for the operating system this binary targets.
#[derive(Debug, Default)]
pub struct HostAppTracker;

impl AppTrackerProvider for HostAppTracker {
    fn current_application(&self) -> Result<Option<ActiveApplication>, AppError> {
        #[cfg(target_os = "macos")]
        {
            wind_macos::current_application()
        }
        #[cfg(target_os = "windows")]
        {
            wind_windows::current_application()
        }
        #[cfg(target_os = "linux")]
        {
            wind_linux::current_application()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Err(AppError::Unsupported {
                reason: "this operating system has no foreground application provider".to_string(),
            })
        }
    }

    fn capability_status(&self) -> CapabilityStatus {
        #[cfg(target_os = "macos")]
        {
            wind_macos::application_status()
        }
        #[cfg(target_os = "windows")]
        {
            wind_windows::application_status()
        }
        #[cfg(target_os = "linux")]
        {
            wind_linux::application_status()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            CapabilityStatus::Unsupported
        }
    }
}

pub fn host_app_tracker() -> Box<dyn AppTrackerProvider> {
    Box::new(HostAppTracker)
}
