//! Accept the system Quit & Reopen request.
//!
//! Screen Recording and Input Monitoring do not apply until this process
//! exits and macOS opens it again. That request arrives as an Apple Event on
//! the main run loop. Wind pumps that loop while it is resident and answers
//! the event, then finishes its own shutdown before the process exits.

#![allow(unsafe_code)]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationDelegate, NSApplicationTerminateReply, NSEventMask,
};
use objc2_core_foundation::CFRunLoop;
use objc2_foundation::{NSDate, NSDefaultRunLoopMode};

static QUIT_NOTIFY: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
static QUIT_LATER: AtomicBool = AtomicBool::new(false);
static REOPEN_SCHEDULED: AtomicBool = AtomicBool::new(false);
static GUIDE_PID: Mutex<Option<u32>> = Mutex::new(None);

/// Remember the permission-guide process so a quit can close its dialog.
pub fn set_permission_guide_pid(pid: Option<u32>) {
    if let Ok(mut slot) = GUIDE_PID.lock() {
        *slot = pid;
    }
}

/// Close the permission dialog so it cannot keep the process on screen.
pub fn dismiss_permission_guide() {
    let pid = GUIDE_PID.lock().ok().and_then(|mut slot| slot.take());
    if let Some(pid) = pid {
        let _ = std::process::Command::new("kill")
            .arg(pid.to_string())
            .status();
    }
}

/// Call `notify` when macOS asks Wind to quit, including Quit & Reopen.
pub fn install_system_quit_handler(notify: impl Fn() + Send + Sync + 'static) {
    let _ = QUIT_NOTIFY.set(Box::new(notify));
    let Some(marker) = MainThreadMarker::new() else {
        return;
    };
    let application = NSApplication::sharedApplication(marker);
    let delegate = QuitDelegate::new(marker);
    let object = ProtocolObject::<dyn NSApplicationDelegate>::from_ref(&*delegate);
    application.setDelegate(Some(object));
    // NSApplication retains the delegate. Leak the extra retain so it stays alive.
    std::mem::forget(delegate);
    application.finishLaunching();
}

/// Quit Wind and open the same app again after this process has exited.
///
/// macOS does not bring a background app back after Quit & Reopen. A helper
/// waits until this process is gone, then opens the bundle if nothing else has.
pub fn reopen_and_quit() {
    schedule_reopen();
    dismiss_permission_guide();
    if let Some(notify) = QUIT_NOTIFY.get() {
        notify();
    }
}

/// Let AppKit deliver a pending Quit & Reopen event.
pub fn pump_main_run_loop(timeout: Duration) {
    let Some(marker) = MainThreadMarker::new() else {
        return;
    };
    let seconds = timeout.as_secs_f64();
    if seconds <= 0.0 {
        return;
    }
    let application = NSApplication::sharedApplication(marker);
    let until = NSDate::dateWithTimeIntervalSinceNow(seconds);
    // SAFETY: NSDefaultRunLoopMode is a Foundation constant. Pumping it on the
    // main thread delivers the quit Apple Event to the delegate.
    unsafe {
        let _ = application.nextEventMatchingMask_untilDate_inMode_dequeue(
            NSEventMask::Any,
            Some(&until),
            NSDefaultRunLoopMode,
            true,
        );
    }
}

fn schedule_reopen() {
    if REOPEN_SCHEDULED.swap(true, Ordering::AcqRel) {
        return;
    }
    let (Ok(executable), Some(bundle)) = (std::env::current_exe(), app_bundle()) else {
        return;
    };
    let mut command = Command::new(executable);
    command
        .env("WIND_REOPEN_WAIT_PID", std::process::id().to_string())
        .env("WIND_REOPEN_BUNDLE", bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let _ = command.spawn();
}

fn app_bundle() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let bundle = executable.parent()?.parent()?.parent()?;
    if bundle.extension().and_then(|extension| extension.to_str()) == Some("app") {
        Some(bundle.to_path_buf())
    } else {
        None
    }
}

/// Tell AppKit it may finish quitting, after Wind has saved its own state.
pub fn finish_system_quit() {
    if !QUIT_LATER.swap(false, Ordering::AcqRel) {
        return;
    }
    let Some(marker) = MainThreadMarker::new() else {
        return;
    };
    NSApplication::sharedApplication(marker).replyToApplicationShouldTerminate(true);
}

struct QuitDelegateIvars;

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = QuitDelegateIvars]
    struct QuitDelegate;

    unsafe impl NSObjectProtocol for QuitDelegate {}

    unsafe impl NSApplicationDelegate for QuitDelegate {
        #[unsafe(method(applicationShouldTerminate:))]
        #[allow(non_snake_case)]
        fn applicationShouldTerminate(
            &self,
            _sender: &NSApplication,
        ) -> NSApplicationTerminateReply {
            schedule_reopen();
            dismiss_permission_guide();
            if let Some(notify) = QUIT_NOTIFY.get() {
                notify();
            }
            QUIT_LATER.store(true, Ordering::Release);
            if let Some(run_loop) = CFRunLoop::current() {
                CFRunLoop::stop(&run_loop);
            }
            NSApplicationTerminateReply::TerminateLater
        }
    }
);

impl QuitDelegate {
    fn new(marker: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(marker).set_ivars(QuitDelegateIvars);
        unsafe { msg_send![super(this), init] }
    }
}
