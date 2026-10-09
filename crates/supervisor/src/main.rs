#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::env;
use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant};

use wind_core::Result;

fn main() {
    if reopen_after_exit() {
        return;
    }
    #[cfg(debug_assertions)]
    if env::var_os("WIND_SERVICE_TEST_MODE").is_some() {
        wind_supervisor::run_service_test_mode();
    }
    let command = match Command::parse(env::args().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            write_stderr(&error.to_string());
            std::process::exit(2);
        }
    };
    let mut runtime = match wind_supervisor::prepare() {
        Ok(runtime) => runtime,
        Err(error) => {
            write_stderr(&error.to_string());
            std::process::exit(1);
        }
    };
    write_stdout(&wind_supervisor::format_report(runtime.report()));
    if let Err(error) = run_command(&mut runtime, &command) {
        write_stderr(&error.to_string());
        runtime.request_shutdown();
        let outcome = runtime.wait();
        write_stdout(&wind_supervisor::format_shutdown(&outcome.shutdown_report));
        std::process::exit(1);
    }
    if command.one_shot() {
        runtime.request_shutdown();
    } else {
        runtime.spawn_permission_guides();
    }
    let outcome = runtime.wait();
    write_stdout(&wind_supervisor::format_shutdown(&outcome.shutdown_report));
    let failed = !outcome.start_failures.is_empty() || !outcome.shutdown_failures.is_empty();
    for error in outcome
        .start_failures
        .iter()
        .chain(outcome.shutdown_failures.iter())
    {
        write_stderr(&error.to_string());
    }
    if failed {
        std::process::exit(1);
    }
}

/// Wait for the previous Wind process to exit, then open the app once.
fn reopen_after_exit() -> bool {
    let Ok(pid_text) = env::var("WIND_REOPEN_WAIT_PID") else {
        return false;
    };
    let Ok(bundle) = env::var("WIND_REOPEN_BUNDLE") else {
        return false;
    };
    let Ok(pid) = pid_text.parse::<u32>() else {
        return false;
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && process_alive(pid) {
        thread::sleep(Duration::from_millis(200));
    }
    thread::sleep(Duration::from_millis(800));
    if !another_wind_is_running() {
        let _ = std::process::Command::new("/usr/bin/open")
            .arg(bundle)
            .status();
    }
    true
}

fn process_alive(pid: u32) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|status| status.success())
}

fn another_wind_is_running() -> bool {
    let me = std::process::id();
    let Ok(output) = std::process::Command::new("/usr/bin/pgrep")
        .args(["-f", "Wind.app/Contents/MacOS/Wind"])
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .filter_map(|token| token.parse::<u32>().ok())
        .any(|pid| pid != me)
}

struct Command {
    screenshots: u32,
    record_seconds: Option<u64>,
    current_application: bool,
    current_url: bool,
    activity_snapshot: bool,
    permissions: bool,
    open_permission: Option<String>,
}

impl Command {
    fn one_shot(&self) -> bool {
        self.screenshots > 0
            || self.record_seconds.is_some()
            || self.current_application
            || self.current_url
            || self.activity_snapshot
            || self.permissions
            || self.open_permission.is_some()
    }

    fn parse(args: impl Iterator<Item = String>) -> Result<Self> {
        let mut screenshots = 0u32;
        let mut record_seconds = None;
        let mut current_application = false;
        let mut current_url = false;
        let mut activity_snapshot = false;
        let mut permissions = false;
        let mut open_permission = None;
        let mut expect_count = false;
        let mut expect_seconds = false;
        let mut expect_permission = false;
        for arg in args {
            if expect_count {
                screenshots = arg.parse::<u32>().map_err(|_| {
                    wind_core::Error::new(
                        "supervisor",
                        "parse_args",
                        "screenshot count is not a number",
                    )
                })?;
                if screenshots == 0 || screenshots > 5 {
                    return Err(wind_core::Error::new(
                        "supervisor",
                        "parse_args",
                        "screenshot count must be from 1 to 5",
                    ));
                }
                expect_count = false;
                continue;
            }
            if expect_permission {
                open_permission = Some(arg);
                expect_permission = false;
                continue;
            }
            if expect_seconds {
                let seconds = arg.parse::<u64>().map_err(|_| {
                    wind_core::Error::new(
                        "supervisor",
                        "parse_args",
                        "record seconds is not a number",
                    )
                })?;
                if !(1..=10).contains(&seconds) {
                    return Err(wind_core::Error::new(
                        "supervisor",
                        "parse_args",
                        "record seconds must be from 1 to 10",
                    ));
                }
                record_seconds = Some(seconds);
                expect_seconds = false;
                continue;
            }
            match arg.as_str() {
                "--capture-screenshot" => {
                    if screenshots == 0 {
                        screenshots = 1;
                    }
                }
                "--count" => expect_count = true,
                "--record-video" => {
                    if record_seconds.is_none() {
                        record_seconds = Some(4);
                    }
                }
                "--seconds" => expect_seconds = true,
                "--current-application" => current_application = true,
                "--current-url" => current_url = true,
                "--activity-snapshot" => activity_snapshot = true,
                "--permissions" => permissions = true,
                "--open-permission" => expect_permission = true,
                "--help" => {
                    write_line(
                        "wind\nwind --capture-screenshot [--count N]\nwind --record-video [--seconds N]\nwind --current-application\nwind --current-url\nwind --activity-snapshot\nwind --permissions\nwind --open-permission CAPABILITY",
                    );
                    std::process::exit(0);
                }
                other => {
                    return Err(wind_core::Error::new(
                        "supervisor",
                        "parse_args",
                        format!("unknown argument {other}"),
                    ));
                }
            }
        }
        if expect_permission {
            return Err(wind_core::Error::new(
                "supervisor",
                "parse_args",
                "a capability id is missing",
            ));
        }
        if expect_count || expect_seconds {
            return Err(wind_core::Error::new(
                "supervisor",
                "parse_args",
                "a numeric value is missing",
            ));
        }
        Ok(Self {
            screenshots,
            record_seconds,
            current_application,
            current_url,
            activity_snapshot,
            permissions,
            open_permission,
        })
    }
}

fn run_command(runtime: &mut wind_supervisor::PreparedRuntime, command: &Command) -> Result<()> {
    for index in 1..=command.screenshots {
        let report = runtime.capture_screenshot()?;
        write_line(&format!(
            "screenshot {index} width={} height={} bytes={} duration_ms={}",
            report.width, report.height, report.byte_len, report.duration_ms
        ));
    }
    if let Some(seconds) = command.record_seconds {
        runtime.start_recording()?;
        let deadline = Instant::now() + Duration::from_secs(seconds);
        while Instant::now() < deadline {
            thread::sleep(Duration::from_millis(100));
        }
        let report = runtime.stop_recording()?;
        write_line(&format!(
            "video width={} height={} bytes={} frames={} duration_ms={}",
            report.width, report.height, report.byte_len, report.frames, report.duration_ms
        ));
    }
    if command.current_application {
        let report = runtime.current_application()?;
        if report.present {
            write_line(&format!(
                "application name={} changed={}",
                report.name, report.changed
            ));
        } else {
            write_line("application none");
        }
    }
    if command.activity_snapshot {
        let report = runtime.activity_snapshot()?;
        write_line(&format!(
            "activity keyboard_events={} mouse_moves={} mouse_clicks={} mouse_scrolls={}",
            report.keyboard_events, report.mouse_moves, report.mouse_clicks, report.mouse_scrolls
        ));
    }
    if command.current_url {
        let report = runtime.current_url()?;
        if report.present {
            write_line(&format!(
                "url browser={} changed={} sanitized={}",
                report.browser, report.changed, report.url
            ));
        } else {
            write_line("url none");
        }
    }
    if command.permissions {
        write_stdout(&runtime.permission_report());
    }
    if let Some(name) = &command.open_permission {
        let capability = wind_core::Capability::parse(name).ok_or_else(|| {
            wind_core::Error::new(
                "supervisor",
                "parse_args",
                format!("unknown capability {name}"),
            )
        })?;
        match runtime.open_permission(capability) {
            Ok(text) => write_stdout(&text),
            Err(error) => {
                return Err(wind_core::Error::new(
                    "supervisor",
                    "open_permission",
                    error.to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn write_stdout(text: &str) {
    let mut out = io::stdout();
    let _ = out.write_all(text.as_bytes());
}

fn write_stderr(text: &str) {
    let mut err = io::stderr();
    let _ = err.write_all(text.as_bytes());
    let _ = err.write_all(b"\n");
}

fn write_line(text: &str) {
    write_stdout(text);
    write_stdout("\n");
}
