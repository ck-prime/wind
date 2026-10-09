//! Host secret store.
//!
//! Tracking services do not call this module. The operating-system
//! implementation stays behind [`wind_storage::SecretStore`].

use wind_storage::{SecretError, SecretStore};

/// The current user's OS secret store.
///
/// An unsupported host returns an error. It does not create a plaintext file.
pub fn host_secret_store() -> Result<Box<dyn SecretStore>, SecretError> {
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(wind_windows::WindowsSecretStore))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(wind_macos::MacosSecretStore))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(wind_linux::LinuxSecretStore))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        wind_storage::report_secret_unavailable();
        Err(SecretError::unavailable(
            "this operating system has no secret store",
        ))
    }
}
