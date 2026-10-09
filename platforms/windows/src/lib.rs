//! Windows desktop capture, foreground application identity, and URL intake.
//!
//! Screenshots use one persistent Windows Graphics Capture session.
//! Video recording uses Windows Graphics Capture through `xcap`.
//! Video frames are encoded to H.264 MP4 with Media Foundation.
//! Application identity uses the foreground window and a limited process query.
//! The current browser URL is read from the address field with UI Automation.
//! This crate does not inject code, read process memory, or require an
//! administrator account.

mod activity;
mod app;
mod classify;
mod encode;
mod screenshot;
mod secret;
mod url;
mod video;
mod wgc;

pub use activity::WindowsActivityProvider;
pub use app::{application_status, current_application};
pub use screenshot::{capture_full_display, screen_capture_status};
pub use secret::WindowsSecretStore;
pub use url::WindowsUrlProvider;
pub use video::{video_capture_status, WindowsVideoProvider};

/// Attach to a parent console when one already exists.
///
/// A double-clicked GUI process has no parent console, so this does nothing
/// and does not create a window. A process started from a terminal can still
/// receive Ctrl+C and Ctrl+Break.
pub fn attach_parent_console() {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
