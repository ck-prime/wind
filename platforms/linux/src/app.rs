//! Foreground application identity on Linux.
//!
//! An X11 session reads `_NET_ACTIVE_WINDOW` and `_NET_WM_PID`, then the
//! executable path from `/proc/<pid>/exe`. Window titles, command lines, and
//! environment variables are not read. Wayland does not publish a supported
//! foreground-application query, so that session is unsupported. A headless
//! session is unavailable. Nothing here requires root.

use std::env;
use std::fs;
use std::path::Path;
use std::time::SystemTime;

use wind_app_tracker::{ActiveApplication, AppError};
use wind_core::CapabilityStatus;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
use x11rb::rust_connection::RustConnection;

pub fn application_status() -> CapabilityStatus {
    match session() {
        Session::Headless => CapabilityStatus::Unavailable,
        Session::Wayland => CapabilityStatus::Unsupported,
        Session::X11 => CapabilityStatus::Available,
    }
}

pub fn current_application() -> Result<Option<ActiveApplication>, AppError> {
    match session() {
        Session::Headless => Err(AppError::Unavailable {
            reason: "no X11 DISPLAY or Wayland session is available".to_string(),
        }),
        Session::Wayland => Err(AppError::Unsupported {
            reason: "Wayland does not expose a supported foreground application query".to_string(),
        }),
        Session::X11 => x11_foreground(),
    }
}

enum Session {
    Headless,
    Wayland,
    X11,
}

fn session() -> Session {
    let display = env::var_os("DISPLAY").is_some();
    let wayland = env::var_os("WAYLAND_DISPLAY").is_some();
    if display {
        Session::X11
    } else if wayland {
        Session::Wayland
    } else {
        Session::Headless
    }
}

fn x11_foreground() -> Result<Option<ActiveApplication>, AppError> {
    let (connection, screen) =
        RustConnection::connect(None).map_err(|error| AppError::Unavailable {
            reason: error.to_string(),
        })?;
    let root = connection
        .setup()
        .roots
        .get(screen)
        .ok_or_else(|| AppError::Unavailable {
            reason: "X11 screen is missing".to_string(),
        })?
        .root;
    let active = intern(&connection, b"_NET_ACTIVE_WINDOW")?;
    let pid_atom = intern(&connection, b"_NET_WM_PID")?;
    let window = property_u32(&connection, root, active, AtomEnum::WINDOW)?;
    if window == 0 {
        return Ok(None);
    }
    let pid = property_u32(&connection, window, pid_atom, AtomEnum::CARDINAL)?;
    if pid == 0 {
        return Err(AppError::Unavailable {
            reason: "foreground window has no process id".to_string(),
        });
    }
    let executable =
        fs::read_link(format!("/proc/{pid}/exe")).map_err(|error| AppError::QueryFailed {
            reason: error.to_string(),
        })?;
    let path = executable.to_string_lossy().to_string();
    let name = Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    Ok(Some(ActiveApplication {
        application_id: path.clone(),
        name,
        executable: Some(path),
        observed_at: SystemTime::now(),
    }))
}

fn intern(connection: &RustConnection, name: &[u8]) -> Result<u32, AppError> {
    connection
        .intern_atom(false, name)
        .map_err(|error| AppError::QueryFailed {
            reason: error.to_string(),
        })?
        .reply()
        .map(|reply| reply.atom)
        .map_err(|error| AppError::QueryFailed {
            reason: error.to_string(),
        })
}

fn property_u32(
    connection: &RustConnection,
    window: u32,
    property: u32,
    kind: impl Into<u32>,
) -> Result<u32, AppError> {
    let reply = connection
        .get_property(false, window, property, kind.into(), 0, 1)
        .map_err(|error| AppError::QueryFailed {
            reason: error.to_string(),
        })?
        .reply()
        .map_err(|error| AppError::QueryFailed {
            reason: error.to_string(),
        })?;
    Ok(reply
        .value32()
        .and_then(|mut values| values.next())
        .unwrap_or(0))
}
