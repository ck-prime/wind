//! Linux desktop capture.
//!
//! X11 uses the X11 connection exported as `DISPLAY`. Wayland does not bypass
//! the compositor: an explicit capture uses the desktop screencast path and may
//! show the system's portal dialog. A health check does not open that dialog.
//! A headless session, with neither `DISPLAY` nor `WAYLAND_DISPLAY`, is
//! unavailable. Nothing here requires root.

mod activity;
mod app;
mod screenshot;
mod secret;
mod url;
mod video;

pub use activity::LinuxActivityProvider;
pub use app::{application_status, current_application};
pub use screenshot::{capture_full_display, screen_capture_status};
pub use secret::LinuxSecretStore;
pub use url::LinuxUrlProvider;
pub use video::{video_capture_status, LinuxVideoProvider};

use std::sync::atomic::{AtomicBool, Ordering};

static CAPTURE_PROVEN: AtomicBool = AtomicBool::new(false);

pub(crate) fn mark_capture_proven() {
    CAPTURE_PROVEN.store(true, Ordering::Release);
}

pub(crate) fn capture_proven() -> bool {
    CAPTURE_PROVEN.load(Ordering::Acquire)
}
