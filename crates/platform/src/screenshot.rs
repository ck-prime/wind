//! Host screenshot provider.
//!
//! Each operating system captures the primary display through its own provider.
//! The supervisor only sees [`ScreenshotProvider`].

use wind_core::CapabilityStatus;
use wind_screenshot::{Screenshot, ScreenshotError, ScreenshotProvider, ScreenshotRequest};

/// Screenshot provider for the operating system this binary was built for.
#[derive(Debug, Default, Clone, Copy)]
pub struct HostScreenshotProvider;

impl HostScreenshotProvider {
    pub fn new() -> Self {
        Self
    }
}

impl ScreenshotProvider for HostScreenshotProvider {
    fn capture(&self, request: &ScreenshotRequest) -> Result<Screenshot, ScreenshotError> {
        let _ = request;
        #[cfg(target_os = "macos")]
        {
            crate::screenshot::macos::capture_full_display()
        }
        #[cfg(target_os = "windows")]
        {
            wind_windows::capture_full_display()
        }
        #[cfg(target_os = "linux")]
        {
            wind_linux::capture_full_display()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Err(ScreenshotError::Unsupported {
                reason: "screenshot capture is not implemented on this operating system"
                    .to_string(),
            })
        }
    }

    fn capability_status(&self) -> CapabilityStatus {
        #[cfg(target_os = "macos")]
        {
            crate::screenshot::macos::screen_capture_status()
        }
        #[cfg(target_os = "windows")]
        {
            wind_windows::screen_capture_status()
        }
        #[cfg(target_os = "linux")]
        {
            wind_linux::screen_capture_status()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            CapabilityStatus::Unsupported
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) mod macos {
    pub(crate) use wind_macos::{
        capture_full_display, screen_capture_permission, screen_capture_status,
    };
}

pub fn host_screenshot_provider() -> Box<dyn ScreenshotProvider> {
    Box::new(HostScreenshotProvider::new())
}

#[cfg(test)]
mod tests {
    use super::HostScreenshotProvider;
    use wind_screenshot::{ScreenshotProvider, ScreenshotRequest};

    #[test]
    fn host_screenshot_uses_the_platform_provider() {
        let provider = HostScreenshotProvider::new();
        let _ = provider.capability_status();
        let _ = ScreenshotRequest::FullDisplay;
    }
}
