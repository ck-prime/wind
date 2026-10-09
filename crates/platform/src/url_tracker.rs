//! Host browser-URL provider.
//!
//! The current URL is read from the foreground browser's accessibility tree.
//! This module does not read browser storage or window titles.

use wind_core::CapabilityStatus;
use wind_url_tracker::{BrowserIntegration, UrlError, UrlObservation, UrlTrackerProvider};

/// URL provider for the operating system this binary targets.
pub struct HostUrlTracker {
    #[cfg(target_os = "windows")]
    inner: wind_windows::WindowsUrlProvider,
    #[cfg(target_os = "macos")]
    inner: wind_macos::MacosUrlProvider,
    #[cfg(target_os = "linux")]
    inner: wind_linux::LinuxUrlProvider,
}

impl HostUrlTracker {
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "windows")]
            inner: wind_windows::WindowsUrlProvider::new(),
            #[cfg(target_os = "macos")]
            inner: wind_macos::MacosUrlProvider::new(),
            #[cfg(target_os = "linux")]
            inner: wind_linux::LinuxUrlProvider::new(),
        }
    }
}

impl Default for HostUrlTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl UrlTrackerProvider for HostUrlTracker {
    fn capability_status(&self) -> CapabilityStatus {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            self.inner.capability_status()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            CapabilityStatus::Unsupported
        }
    }

    fn integration(&self) -> BrowserIntegration {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            self.inner.integration()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            BrowserIntegration::Unsupported
        }
    }

    fn current_url(&self) -> Result<Option<UrlObservation>, UrlError> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            self.inner.current_url()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Err(UrlError::Unsupported {
                reason: "this operating system has no browser url provider".to_string(),
            })
        }
    }

    fn start(&mut self) -> Result<(), UrlError> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            self.inner.start()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Err(UrlError::Unsupported {
                reason: "this operating system has no browser url provider".to_string(),
            })
        }
    }

    fn stop(&mut self) -> Result<(), UrlError> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            self.inner.stop()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Ok(())
        }
    }
}

pub fn host_url_tracker() -> Box<dyn UrlTrackerProvider> {
    Box::new(HostUrlTracker::new())
}
