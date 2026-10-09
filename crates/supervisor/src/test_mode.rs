#![cfg(debug_assertions)]
// Debug-only stand-in for a service executable.
//!
//! `WIND_SERVICE_TEST_MODE` selects `serve`, `crash`, or `fail`. Release builds
//! do not include this entry point.

use std::io::{self, BufRead, Write};

use wind_core::{
    decode_request, encode_response, HealthMessage, ReadyMessage, ServiceId, ServiceRequest,
    ServiceResponse, Version, SERVICE_PROTOCOL_VERSION,
};

#[cfg(debug_assertions)]
pub fn run_service_test_mode() -> ! {
    let mode = std::env::var("WIND_SERVICE_TEST_MODE").unwrap_or_else(|_| "serve".to_string());
    if mode == "fail" {
        std::process::exit(1);
    }
    let service_id = std::env::var("WIND_SERVICE_TEST_ID")
        .ok()
        .and_then(|value| ServiceId::parse(&value))
        .unwrap_or(ServiceId::AppTracker);
    let version = std::env::var("WIND_SERVICE_TEST_VERSION")
        .ok()
        .and_then(|value| Version::parse(&value).ok())
        .unwrap_or_else(|| Version::new(0, 1, 0));
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let mut stdout = io::stdout();
    while let Some(Ok(line)) = lines.next() {
        let Ok(request) = decode_request(&line) else {
            continue;
        };
        match request {
            ServiceRequest::Hello => {
                let _ = write_line(
                    &mut stdout,
                    &ServiceResponse::Ready(ReadyMessage {
                        service_id,
                        version,
                        protocol_version: SERVICE_PROTOCOL_VERSION,
                        capabilities: Vec::new(),
                    }),
                );
                if mode == "crash" {
                    std::process::exit(0);
                }
            }
            ServiceRequest::Health => {
                let _ = write_line(
                    &mut stdout,
                    &ServiceResponse::Health(HealthMessage {
                        status: wind_core::HealthStatus::Healthy,
                        capability: wind_core::CapabilityStatus::Available,
                        detail: "running".to_string(),
                    }),
                );
            }
            ServiceRequest::CurrentApplication => {
                let _ = write_line(
                    &mut stdout,
                    &ServiceResponse::Application(wind_core::ApplicationMessage {
                        present: false,
                        application_id: String::new(),
                        name: String::new(),
                        executable: String::new(),
                    }),
                );
            }
            ServiceRequest::Shutdown => {
                let _ = write_line(&mut stdout, &ServiceResponse::Bye);
                std::process::exit(0);
            }
        }
    }
    std::process::exit(0);
}

#[cfg(debug_assertions)]
fn write_line(stdout: &mut impl Write, response: &ServiceResponse) -> io::Result<()> {
    writeln!(stdout, "{}", encode_response(response))?;
    stdout.flush()
}
