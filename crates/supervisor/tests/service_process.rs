//! Out-of-process launch, crash, and shutdown.

use std::path::PathBuf;
use std::time::Duration;

use wind_core::{ServiceId, Version};
use wind_supervisor::{LaunchSpec, ServiceProcess};

fn service_executable(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("wind-service-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let name = if cfg!(windows) {
        "wind-app-tracker.exe"
    } else {
        "wind-app-tracker"
    };
    let destination = root.join(name);
    std::fs::copy(env!("CARGO_BIN_EXE_wind"), &destination).unwrap();
    destination
}

fn launch(
    executable: PathBuf,
    mode: &str,
    version: Version,
) -> Result<ServiceProcess, wind_core::Error> {
    ServiceProcess::launch(LaunchSpec {
        service_id: ServiceId::AppTracker,
        version,
        executable,
        test_mode: Some(mode.to_string()),
    })
}

#[test]
fn service_reports_metadata_and_shuts_down() {
    let executable = service_executable("serve");
    let mut process = launch(executable, "serve", Version::new(0, 1, 0)).unwrap();
    let (health, capability) = process.health().unwrap();
    assert_eq!(health, wind_core::HealthStatus::Healthy);
    assert_eq!(capability, wind_core::CapabilityStatus::Available);
    process.shutdown();
    assert!(!process.is_running());
}

#[test]
fn startup_failure_is_rejected() {
    let executable = service_executable("fail");
    let launched = launch(executable, "fail", Version::new(0, 1, 0));
    let error = match launched {
        Ok(_) => panic!("failed startup must be rejected"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("app_tracker"), "{error}");
}

#[test]
fn crash_loop_stops_restarting() {
    let executable = service_executable("crash");
    let mut process = launch(executable, "crash", Version::new(0, 1, 0)).unwrap();
    let mut failed = false;
    for _ in 0..5 {
        if process.recover().is_err() {
            failed = true;
            break;
        }
        thread_sleep();
    }
    assert!(failed);
    process.shutdown();
}

fn thread_sleep() {
    std::thread::sleep(Duration::from_millis(10));
}
