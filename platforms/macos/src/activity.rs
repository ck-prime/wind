//! Aggregate input counts on macOS.
//!
//! A session event tap in listen-only mode classifies events on one thread.
//! The callback reads the event type and returns the event unchanged. It does
//! not read the key code or the pointer location. `CGPreflightListenEventAccess`
//! reports readiness and does not prompt. `CGRequestListenEventAccess` is not
//! called. A missing grant leaves the service running with permission required.

#![allow(unsafe_code)]

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use objc2_core_foundation::{CFMachPort, CFRunLoop, CFRunLoopMode};
use objc2_core_graphics::{
    CGEvent, CGEventMaskBit, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType, CGPreflightListenEventAccess,
};
use wind_activity::{ActivityCounters, ActivityError, ActivityProvider, ActivitySnapshot};
use wind_core::{Capability, CapabilityStatus, PermissionState, PermissionStatus};

thread_local! {
    static COUNTERS: Cell<*const ActivityCounters> = const { Cell::new(std::ptr::null()) };
}

const KEY_DOWN: u32 = 10;
const MOUSE_MOVED: u32 = 5;
const LEFT_DRAG: u32 = 6;
const RIGHT_DRAG: u32 = 7;
const OTHER_DRAG: u32 = 27;
const LEFT_DOWN: u32 = 1;
const RIGHT_DOWN: u32 = 3;
const OTHER_DOWN: u32 = 25;
const SCROLL: u32 = 22;
const SESSION_TAP: u32 = 1;
const HEAD_INSERT: u32 = 0;
const LISTEN_ONLY: u32 = 1;

pub struct MacosActivityProvider {
    counters: ActivityCounters,
    worker: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    run_loop: Arc<AtomicPtr<CFRunLoop>>,
    listening: bool,
    permission_required: bool,
    unavailable_reason: Option<String>,
}

impl Default for MacosActivityProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MacosActivityProvider {
    pub fn new() -> Self {
        Self {
            counters: ActivityCounters::new(),
            worker: None,
            stop: Arc::new(AtomicBool::new(false)),
            run_loop: Arc::new(AtomicPtr::new(std::ptr::null_mut())),
            listening: false,
            permission_required: false,
            unavailable_reason: None,
        }
    }
}

impl Drop for MacosActivityProvider {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Input Monitoring readiness.
///
/// The preflight function returns a boolean and does not prompt. `false` is
/// `NotDetermined` because the API cannot tell a first prompt from a denial.
pub fn input_monitoring_permission() -> PermissionStatus {
    if CGPreflightListenEventAccess() {
        PermissionStatus::new(
            Capability::InputMonitoring,
            PermissionState::Granted,
            "CGPreflightListenEventAccess reports input monitoring access",
        )
    } else {
        PermissionStatus::new(
            Capability::InputMonitoring,
            PermissionState::NotDetermined,
            "CGPreflightListenEventAccess does not report access and does not prompt",
        )
    }
}

impl ActivityProvider for MacosActivityProvider {
    fn start(&mut self) -> Result<(), ActivityError> {
        if self.worker.is_some() {
            return Ok(());
        }
        if !CGPreflightListenEventAccess() {
            self.permission_required = true;
            self.listening = false;
            return Ok(());
        }
        self.permission_required = false;
        self.unavailable_reason = None;
        self.stop.store(false, Ordering::Release);
        let counters = self.counters.clone();
        let stop = Arc::clone(&self.stop);
        let run_loop = Arc::clone(&self.run_loop);
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = thread::Builder::new()
            .name("wind-activity-input".to_string())
            .spawn(move || listen(counters, stop, run_loop, sender))
            .map_err(|error| ActivityError::ProviderFailed {
                reason: error.to_string(),
            })?;
        self.worker = Some(worker);
        match receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => {
                self.listening = true;
                Ok(())
            }
            Ok(Err(reason)) => {
                self.listening = false;
                if reason.contains("permission") {
                    self.permission_required = true;
                } else {
                    self.unavailable_reason = Some(reason);
                }
                self.join_worker();
                Ok(())
            }
            Err(_) => {
                self.stop.store(true, Ordering::Release);
                self.stop_run_loop();
                self.join_worker();
                Err(ActivityError::ProviderFailed {
                    reason: "activity listener did not start".to_string(),
                })
            }
        }
    }

    fn snapshot(&mut self) -> Result<ActivitySnapshot, ActivityError> {
        if self.permission_required {
            return Err(ActivityError::PermissionRequired {
                reason: "input monitoring permission is required".to_string(),
            });
        }
        if let Some(reason) = &self.unavailable_reason {
            return Err(ActivityError::Unavailable {
                reason: reason.clone(),
            });
        }
        if !self.listening {
            return Err(ActivityError::Unavailable {
                reason: "activity listener is not running".to_string(),
            });
        }
        Ok(self.counters.snapshot())
    }

    fn capability_status(&self) -> CapabilityStatus {
        if self.permission_required || !CGPreflightListenEventAccess() {
            CapabilityStatus::PermissionRequired
        } else if self.unavailable_reason.is_some() || !self.listening {
            CapabilityStatus::Unavailable
        } else {
            CapabilityStatus::Available
        }
    }

    fn stop(&mut self) -> Result<(), ActivityError> {
        self.stop.store(true, Ordering::Release);
        self.stop_run_loop();
        self.join_worker();
        self.listening = false;
        Ok(())
    }
}

impl MacosActivityProvider {
    fn stop_run_loop(&self) {
        let pointer = self.run_loop.load(Ordering::Acquire);
        if !pointer.is_null() {
            unsafe {
                (*pointer).stop();
            }
        }
    }

    fn join_worker(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn listen(
    counters: ActivityCounters,
    stop: Arc<AtomicBool>,
    run_loop_slot: Arc<AtomicPtr<CFRunLoop>>,
    sender: std::sync::mpsc::Sender<Result<(), String>>,
) {
    COUNTERS.with(|slot| slot.set(&counters));
    let mask = event_mask();
    let tap = unsafe {
        CGEvent::tap_create(
            CGEventTapLocation(SESSION_TAP),
            CGEventTapPlacement(HEAD_INSERT),
            CGEventTapOptions(LISTEN_ONLY),
            mask,
            Some(tap_callback),
            std::ptr::null_mut(),
        )
    };
    let Some(tap) = tap else {
        COUNTERS.with(|slot| slot.set(std::ptr::null()));
        let _ = sender.send(Err(
            "input monitoring event tap was not created; permission may be required".to_string(),
        ));
        return;
    };
    let Some(source) = CFMachPort::new_run_loop_source(None, Some(&tap), 0) else {
        COUNTERS.with(|slot| slot.set(std::ptr::null()));
        let _ = sender.send(Err("event tap run loop source was not created".to_string()));
        return;
    };
    let current = CFRunLoop::get_current();
    current.add_source(Some(&source), Some(unsafe { kCFRunLoopCommonModes }));
    run_loop_slot.store(
        objc2_core_foundation::CFRetained::as_ptr(&current) as *mut CFRunLoop,
        Ordering::Release,
    );
    let _ = sender.send(Ok(()));
    while !stop.load(Ordering::Acquire) {
        let _ = CFRunLoop::run_in_mode(Some(unsafe { kCFRunLoopDefaultMode }), 0.2, true);
    }
    run_loop_slot.store(std::ptr::null_mut(), Ordering::Release);
    COUNTERS.with(|slot| slot.set(std::ptr::null()));
}

fn event_mask() -> u64 {
    let mut mask = 0u64;
    for value in [
        KEY_DOWN,
        MOUSE_MOVED,
        LEFT_DRAG,
        RIGHT_DRAG,
        OTHER_DRAG,
        LEFT_DOWN,
        RIGHT_DOWN,
        OTHER_DOWN,
        SCROLL,
    ] {
        mask |= CGEventMaskBit(CGEventType(value));
    }
    mask
}

unsafe extern "C-unwind" fn tap_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: NonNull<CGEvent>,
    _user_info: *mut c_void,
) -> *mut CGEvent {
    COUNTERS.with(|slot| {
        let pointer = slot.get();
        if !pointer.is_null() {
            classify(event_type.0, unsafe { &*pointer });
        }
    });
    event.as_ptr()
}

fn classify(event_type: u32, counters: &ActivityCounters) {
    match event_type {
        KEY_DOWN => counters.record_keyboard_event(),
        MOUSE_MOVED | LEFT_DRAG | RIGHT_DRAG | OTHER_DRAG => counters.record_mouse_move(),
        LEFT_DOWN | RIGHT_DOWN | OTHER_DOWN => counters.record_mouse_click(),
        SCROLL => counters.record_mouse_scroll(),
        _ => {}
    }
}

unsafe extern "C" {
    static kCFRunLoopCommonModes: &'static CFRunLoopMode;
    static kCFRunLoopDefaultMode: &'static CFRunLoopMode;
}
