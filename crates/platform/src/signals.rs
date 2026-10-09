//! Process termination notifications.
//!
//! `ctrlc` is the smallest mature crate that handles both Unix `SIGINT` /
//! `SIGTERM` and Windows console control events without this workspace using
//! `unsafe`. The standard library can catch neither portably, and
//! `unsafe_code` is forbidden here.
//!
//! The installed callback runs on `ctrlc`'s thread, not inside a signal
//! handler. Callers must only notify their shutdown coordinator. Cleanup stays
//! on the runtime thread. This does not install a Windows service, a
//! LaunchAgent, or a systemd unit.

use crate::paths::PlatformError;

/// Ask `notify` when the console or the operating system requests termination.
///
/// On Unix this is `SIGINT`, `SIGTERM`, and `SIGHUP`. That requires the `ctrlc`
/// `termination` feature. On Windows this is a console
/// control event such as Ctrl+C or console close. It is not a Service Control
/// Manager handler.
///
/// The process can install only one handler. Tests must not call this.
pub fn install_termination_handler<F>(notify: F) -> Result<(), PlatformError>
where
    F: FnMut() + Send + 'static,
{
    #[cfg(windows)]
    wind_windows::attach_parent_console();
    ctrlc::set_handler(notify).map_err(|error| PlatformError::Signal {
        reason: error.to_string(),
    })
}
