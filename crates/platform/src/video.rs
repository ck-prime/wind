//! Host video provider.
//!
//! Each operating system records the primary display through its own provider.

use wind_core::CapabilityStatus;
use wind_video::{VideoConfig, VideoError, VideoProvider, VideoResult};

/// Video provider for the operating system this binary was built for.
pub struct HostVideoProvider {
    #[cfg(target_os = "windows")]
    inner: wind_windows::WindowsVideoProvider,
    #[cfg(target_os = "macos")]
    inner: wind_macos::MacosVideoProvider,
    #[cfg(target_os = "linux")]
    inner: wind_linux::LinuxVideoProvider,
}

impl HostVideoProvider {
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "windows")]
            inner: wind_windows::WindowsVideoProvider::new(),
            #[cfg(target_os = "macos")]
            inner: wind_macos::MacosVideoProvider::new(),
            #[cfg(target_os = "linux")]
            inner: wind_linux::LinuxVideoProvider::new(),
        }
    }
}

impl Default for HostVideoProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoProvider for HostVideoProvider {
    fn capability_status(&self) -> CapabilityStatus {
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        {
            self.inner.capability_status()
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            CapabilityStatus::Unsupported
        }
    }

    fn start(&mut self, output: &std::path::Path, config: &VideoConfig) -> Result<(), VideoError> {
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        {
            self.inner.start(output, config)
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            let _ = (output, config);
            Err(VideoError::Unsupported {
                reason: "video capture is not implemented on this operating system".to_string(),
            })
        }
    }

    fn stop(&mut self) -> Result<VideoResult, VideoError> {
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        {
            self.inner.stop()
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            Err(VideoError::Unsupported {
                reason: "video capture is not implemented on this operating system".to_string(),
            })
        }
    }

    fn is_recording(&self) -> bool {
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        {
            self.inner.is_recording()
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            false
        }
    }
}

pub fn host_video_provider() -> Box<dyn VideoProvider> {
    Box::new(HostVideoProvider::new())
}
