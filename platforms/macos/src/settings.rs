//! Open System Settings for a privacy pane.
//!
//! This runs only when the caller asks. It does not read TCC, prompt, or
//! change a permission. The destination URL is chosen by `wind-platform`.

use std::process::Command;

/// Open `url` with the system `open` tool. No shell is used.
pub fn open_system_settings(url: &str) -> Result<(), ()> {
    match Command::new("open").arg(url).status() {
        Ok(status) if status.success() => Ok(()),
        _ => Err(()),
    }
}
