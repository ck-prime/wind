//! Aggregate input counts on Windows.
//!
//! `WH_KEYBOARD_LL` and `WH_MOUSE_LL` run on one listener thread in this
//! process. They do not require a DLL, a driver, or an administrator account.
//! The callback reads only the message id. It does not read the hook struct,
//! so virtual keys, scan codes, characters, and pointer coordinates are never
//! copied out. Each accepted message increments one counter and is discarded.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use wind_activity::{ActivityCounters, ActivityError, ActivityProvider, ActivitySnapshot};
use wind_core::CapabilityStatus;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, PeekMessageW, PostThreadMessageW, SetWindowsHookExW,
    UnhookWindowsHookEx, MSG, PM_NOREMOVE, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_MBUTTONDOWN, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN,
    WM_SYSKEYDOWN, WM_XBUTTONDOWN,
};

thread_local! {
    static COUNTERS: Cell<*const ActivityCounters> = const { Cell::new(std::ptr::null()) };
}

/// Windows aggregate activity provider.
///
/// Counters belong to this instance. The listener thread keeps a thread-local
/// pointer to them because a low-level hook callback has no user-data argument.
pub struct WindowsActivityProvider {
    counters: ActivityCounters,
    worker: Option<JoinHandle<()>>,
    thread_id: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    listening: bool,
    unavailable: Option<String>,
}

impl Default for WindowsActivityProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsActivityProvider {
    pub fn new() -> Self {
        Self {
            counters: ActivityCounters::new(),
            worker: None,
            thread_id: Arc::new(AtomicU32::new(0)),
            stop: Arc::new(AtomicBool::new(false)),
            listening: false,
            unavailable: None,
        }
    }
}

impl Drop for WindowsActivityProvider {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

impl ActivityProvider for WindowsActivityProvider {
    fn start(&mut self) -> Result<(), ActivityError> {
        if self.worker.is_some() {
            return Ok(());
        }
        self.stop.store(false, Ordering::Release);
        self.unavailable = None;
        let counters = self.counters.clone();
        let stop = Arc::clone(&self.stop);
        let thread_id = Arc::clone(&self.thread_id);
        let (sender, receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("wind-activity-input".to_string())
            .spawn(move || listen(counters, stop, thread_id, sender))
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
                self.unavailable = Some(reason);
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
        if let Some(reason) = &self.unavailable {
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
        if self.unavailable.is_some() || !self.listening {
            CapabilityStatus::Unavailable
        } else {
            CapabilityStatus::Available
        }
    }

    fn stop(&mut self) -> Result<(), ActivityError> {
        self.stop.store(true, Ordering::Release);
        let id = self.thread_id.load(Ordering::Acquire);
        if id != 0 {
            unsafe {
                let _ = PostThreadMessageW(id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        self.join_worker();
        self.listening = false;
        self.thread_id.store(0, Ordering::Release);
        Ok(())
    }
}

impl WindowsActivityProvider {
    fn join_worker(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn listen(
    counters: ActivityCounters,
    stop: Arc<AtomicBool>,
    thread_id: Arc<AtomicU32>,
    sender: mpsc::Sender<Result<(), String>>,
) {
    COUNTERS.with(|slot| slot.set(&counters));
    let keyboard = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), None, 0) };
    let mouse = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0) };
    let (keyboard, mouse) = match (keyboard, mouse) {
        (Ok(keyboard), Ok(mouse)) => (keyboard, mouse),
        (keyboard, mouse) => {
            if let Ok(hook) = keyboard {
                unsafe {
                    let _ = UnhookWindowsHookEx(hook);
                }
            }
            if let Ok(hook) = mouse {
                unsafe {
                    let _ = UnhookWindowsHookEx(hook);
                }
            }
            let _ = sender.send(Err(
                "low-level input hooks could not be installed".to_string()
            ));
            COUNTERS.with(|slot| slot.set(std::ptr::null()));
            return;
        }
    };
    let mut message = MSG::default();
    unsafe {
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
    }
    thread_id.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
    let _ = sender.send(Ok(()));
    while !stop.load(Ordering::Acquire) {
        let next = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if !next.as_bool() {
            break;
        }
    }
    unsafe {
        let _ = UnhookWindowsHookEx(keyboard);
        let _ = UnhookWindowsHookEx(mouse);
    }
    COUNTERS.with(|slot| slot.set(std::ptr::null()));
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == 0 {
        let message = wparam.0 as u32;
        if message == WM_KEYDOWN || message == WM_SYSKEYDOWN {
            record_keyboard();
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == 0 {
        let message = wparam.0 as u32;
        if message == WM_MOUSEMOVE {
            record_move();
        } else if message == WM_LBUTTONDOWN
            || message == WM_RBUTTONDOWN
            || message == WM_MBUTTONDOWN
            || message == WM_XBUTTONDOWN
        {
            record_click();
        } else if message == WM_MOUSEWHEEL || message == WM_MOUSEHWHEEL {
            record_scroll();
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

fn record_keyboard() {
    with_counters(ActivityCounters::record_keyboard_event);
}

fn record_move() {
    with_counters(ActivityCounters::record_mouse_move);
}

fn record_click() {
    with_counters(ActivityCounters::record_mouse_click);
}

fn record_scroll() {
    with_counters(ActivityCounters::record_mouse_scroll);
}

fn with_counters(record: fn(&ActivityCounters)) {
    COUNTERS.with(|slot| {
        let pointer = slot.get();
        if !pointer.is_null() {
            record(unsafe { &*pointer });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::WindowsActivityProvider;
    use std::thread;
    use std::time::Duration;
    use wind_activity::ActivityProvider;
    use wind_core::CapabilityStatus;

    #[test]
    fn listener_starts_snapshots_and_stops() {
        let mut provider = WindowsActivityProvider::new();
        provider.start().unwrap();
        assert_eq!(provider.capability_status(), CapabilityStatus::Available);
        let first = provider.snapshot().unwrap();
        thread::sleep(Duration::from_millis(30));
        let second = provider.snapshot().unwrap();
        assert!(second.keyboard_events >= first.keyboard_events);
        assert!(second.mouse_moves >= first.mouse_moves);
        assert!(second.mouse_clicks >= first.mouse_clicks);
        assert!(second.mouse_scrolls >= first.mouse_scrolls);
        assert!(second.observed_at >= first.observed_at);
        provider.stop().unwrap();
        provider.stop().unwrap();
        assert_eq!(provider.capability_status(), CapabilityStatus::Unavailable);
        provider.start().unwrap();
        assert_eq!(provider.capability_status(), CapabilityStatus::Available);
        provider.stop().unwrap();
    }

    /// One key-down, two moves, one click, and one wheel event.
    ///
    /// Ignored so an ordinary test run does not click the foreground window.
    /// Run it with `--ignored` on a machine where that click is acceptable.
    #[ignore]
    #[test]
    fn injected_activity_increases_counters_without_printing_input() {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            keybd_event, mouse_event, KEYEVENTF_KEYUP, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
            MOUSEEVENTF_MOVE, MOUSEEVENTF_WHEEL, VIRTUAL_KEY,
        };
        use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos};

        let mut provider = WindowsActivityProvider::new();
        provider.start().unwrap();
        let before = provider.snapshot().unwrap();
        let mut cursor = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut cursor);
            keybd_event(VIRTUAL_KEY(0x10).0 as u8, 0, Default::default(), 0);
            keybd_event(VIRTUAL_KEY(0x10).0 as u8, 0, KEYEVENTF_KEYUP, 0);
            mouse_event(MOUSEEVENTF_MOVE, 4, 0, 0, 0);
            mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
            mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
            mouse_event(MOUSEEVENTF_WHEEL, 0, 0, 120, 0);
            mouse_event(MOUSEEVENTF_MOVE, -4, 0, 0, 0);
            let _ = SetCursorPos(cursor.x, cursor.y);
        }
        thread::sleep(Duration::from_millis(200));
        let after = provider.snapshot().unwrap();
        provider.stop().unwrap();
        assert!(after.keyboard_events > before.keyboard_events);
        assert!(after.mouse_moves > before.mouse_moves);
        assert!(after.mouse_clicks > before.mouse_clicks);
        assert!(after.mouse_scrolls > before.mouse_scrolls);
    }
}
