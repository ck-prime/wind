//! Host aggregate activity provider.
//!
//! The service sees [`ActivityProvider`]. Hook handles, event taps, and X11
//! events stay in the platform crate for this operating system.

use wind_activity::ActivityProvider;

/// Activity provider for the operating system this binary targets.
pub fn host_activity_provider() -> Box<dyn ActivityProvider> {
    #[cfg(target_os = "macos")]
    {
        Box::new(wind_macos::MacosActivityProvider::new())
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(wind_windows::WindowsActivityProvider::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(wind_linux::LinuxActivityProvider::new())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Box::new(wind_activity::FakeActivityProvider::new(
            wind_activity::FakeActivityOutcome::Unsupported,
        ))
    }
}
