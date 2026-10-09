//! Aggregate input counts on Linux.
//!
//! An X11 session selects XInput2 raw events on the master devices and counts
//! key presses, pointer motion, button presses, and wheel buttons. The button
//! number is used only to tell a click from a scroll, then discarded. Key
//! codes and axis values are not stored. Wayland has no supported global input
//! feed, so that session is unsupported. A headless session is unavailable.
//! Nothing here requires root or reads compositor internals.

use std::env;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use wind_activity::{ActivityCounters, ActivityError, ActivityProvider, ActivitySnapshot};
use wind_core::CapabilityStatus;
use x11rb::connection::Connection;
use x11rb::protocol::xinput::{ConnectionExt as _, Device, EventMask, XIEventMask};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

pub struct LinuxActivityProvider {
    counters: ActivityCounters,
    worker: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    status: CapabilityStatus,
    unavailable_reason: Option<String>,
}

impl Default for LinuxActivityProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxActivityProvider {
    pub fn new() -> Self {
        Self {
            counters: ActivityCounters::new(),
            worker: None,
            stop: Arc::new(AtomicBool::new(false)),
            status: session_status(),
            unavailable_reason: None,
        }
    }
}

impl Drop for LinuxActivityProvider {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

impl ActivityProvider for LinuxActivityProvider {
    fn start(&mut self) -> Result<(), ActivityError> {
        if self.worker.is_some() {
            return Ok(());
        }
        self.status = session_status();
        if self.status != CapabilityStatus::Available {
            return Ok(());
        }
        self.stop.store(false, Ordering::Release);
        let counters = self.counters.clone();
        let stop = Arc::clone(&self.stop);
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = thread::Builder::new()
            .name("wind-activity-input".to_string())
            .spawn(move || listen(counters, stop, sender))
            .map_err(|error| ActivityError::ProviderFailed {
                reason: error.to_string(),
            })?;
        self.worker = Some(worker);
        match receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(reason)) => {
                self.status = CapabilityStatus::Unavailable;
                self.unavailable_reason = Some(reason);
                self.join_worker();
                Ok(())
            }
            Err(_) => {
                self.stop.store(true, Ordering::Release);
                self.join_worker();
                Err(ActivityError::ProviderFailed {
                    reason: "activity listener did not start".to_string(),
                })
            }
        }
    }

    fn snapshot(&mut self) -> Result<ActivitySnapshot, ActivityError> {
        match self.status {
            CapabilityStatus::Available => Ok(self.counters.snapshot()),
            CapabilityStatus::Unsupported => Err(ActivityError::Unsupported {
                reason: "Wayland does not expose supported global input events".to_string(),
            }),
            _ => Err(ActivityError::Unavailable {
                reason: self
                    .unavailable_reason
                    .clone()
                    .unwrap_or_else(|| "input monitoring is unavailable".to_string()),
            }),
        }
    }

    fn capability_status(&self) -> CapabilityStatus {
        self.status
    }

    fn stop(&mut self) -> Result<(), ActivityError> {
        self.stop.store(true, Ordering::Release);
        self.join_worker();
        Ok(())
    }
}

impl LinuxActivityProvider {
    fn join_worker(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn session_status() -> CapabilityStatus {
    let display = env::var_os("DISPLAY").is_some();
    let wayland = env::var_os("WAYLAND_DISPLAY").is_some();
    if display {
        CapabilityStatus::Available
    } else if wayland {
        CapabilityStatus::Unsupported
    } else {
        CapabilityStatus::Unavailable
    }
}

fn listen(
    counters: ActivityCounters,
    stop: Arc<AtomicBool>,
    sender: std::sync::mpsc::Sender<Result<(), String>>,
) {
    let connection = match RustConnection::connect(None) {
        Ok((connection, _)) => connection,
        Err(error) => {
            let _ = sender.send(Err(error.to_string()));
            return;
        }
    };
    if let Err(error) = select_raw_events(&connection) {
        let _ = sender.send(Err(error));
        return;
    }
    let _ = sender.send(Ok(()));
    while !stop.load(Ordering::Acquire) {
        match connection.poll_for_event() {
            Ok(Some(event)) => classify(&counters, event),
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(_) => break,
        }
    }
}

fn select_raw_events(connection: &RustConnection) -> Result<(), String> {
    connection
        .xinput_xi_query_version(2, 2)
        .map_err(|error| error.to_string())?
        .reply()
        .map_err(|error| error.to_string())?;
    let screen = connection
        .setup()
        .roots
        .first()
        .ok_or_else(|| "X11 screen is missing".to_string())?;
    let bits = XIEventMask::RAW_KEY_PRESS | XIEventMask::RAW_MOTION | XIEventMask::RAW_BUTTON_PRESS;
    let mask = EventMask {
        deviceid: Device::ALL_MASTER.into(),
        mask: vec![bits],
    };
    connection
        .xinput_xi_select_events(screen.root, &[mask])
        .map_err(|error| error.to_string())?
        .check()
        .map_err(|error| error.to_string())?;
    connection.flush().map_err(|error| error.to_string())
}

fn classify(counters: &ActivityCounters, event: Event) {
    match event {
        Event::XinputRawKeyPress(_) => counters.record_keyboard_event(),
        Event::XinputRawMotion(_) => counters.record_mouse_move(),
        Event::XinputRawButtonPress(event) => {
            if matches!(event.detail, 4 | 5 | 6 | 7) {
                counters.record_mouse_scroll();
            } else {
                counters.record_mouse_click();
            }
        }
        _ => {}
    }
}
