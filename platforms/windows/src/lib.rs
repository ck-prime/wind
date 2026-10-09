//! Windows desktop capture, foreground application identity, and URL intake.
//!
//! Screenshots use one persistent Windows Graphics Capture session.
//! Video recording uses Windows Graphics Capture through `xcap`.
//! Video frames are encoded to H.264 MP4 with Media Foundation.
//! Application identity uses the foreground window and a limited process query.
//! The current browser URL is read from the address field with UI Automation.
//! This crate does not inject code, read process memory, or require an
//! administrator account.

#[cfg(windows)]
mod activity;
#[cfg(windows)]
mod app;
#[cfg(windows)]
mod classify;
#[cfg(windows)]
mod encode;
#[cfg(windows)]
mod screenshot;
#[cfg(windows)]
mod secret;
#[cfg(windows)]
mod url;
#[cfg(windows)]
mod video;
#[cfg(windows)]
mod wgc;

#[cfg(windows)]
pub use activity::WindowsActivityProvider;
#[cfg(windows)]
pub use app::{application_status, current_application};
#[cfg(windows)]
pub use screenshot::{capture_full_display, screen_capture_status};
#[cfg(windows)]
pub use secret::WindowsSecretStore;
#[cfg(windows)]
pub use url::WindowsUrlProvider;
#[cfg(windows)]
pub use video::{video_capture_status, WindowsVideoProvider};

/// Attach to a parent console when one already exists.
///
/// A double-clicked GUI process has no parent console, so this does nothing
/// and does not create a window. A process started from a terminal can still
/// receive Ctrl+C and Ctrl+Break. Non-Windows builds have no console to attach.
pub fn attach_parent_console() {
    #[cfg(windows)]
    {
        use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
        unsafe {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}
