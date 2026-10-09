//! Foreground application service process.
//!
//! The supervisor starts this executable and speaks the service protocol on
//! stdin and stdout. Logs stay on stderr. This process does not open the
//! tracking database, read secrets, or grant permissions.

use std::io::{self, BufRead, Write};

use wind_core::{
    decode_request, encode_response, ApplicationMessage, Capability, HealthMessage, ReadyMessage,
    ServiceId, ServiceRequest, ServiceResponse, Version, SERVICE_PROTOCOL_VERSION,
};
use wind_platform::host_app_tracker;

fn main() {
    let code = match serve() {
        Ok(()) => 0,
        Err(()) => 1,
    };
    std::process::exit(code);
}

fn serve() -> Result<(), ()> {
    let version = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|_| ())?;
    let provider = host_app_tracker();
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let mut stdout = io::stdout();
    loop {
        let Some(line) = lines.next() else {
            return Ok(());
        };
        let line = line.map_err(|_| ())?;
        let request = match decode_request(&line) {
            Ok(request) => request,
            Err(error) => {
                write_line(&mut stdout, &ServiceResponse::Error(error))?;
                continue;
            }
        };
        match request {
            ServiceRequest::Hello => write_line(
                &mut stdout,
                &ServiceResponse::Ready(ReadyMessage {
                    service_id: ServiceId::AppTracker,
                    version,
                    protocol_version: SERVICE_PROTOCOL_VERSION,
                    capabilities: vec![Capability::ActiveApplication],
                }),
            )?,
            ServiceRequest::Health => {
                let capability = provider.capability_status();
                write_line(
                    &mut stdout,
                    &ServiceResponse::Health(HealthMessage {
                        status: wind_core::HealthStatus::Healthy,
                        capability,
                        detail: "running".to_string(),
                    }),
                )?;
            }
            ServiceRequest::CurrentApplication => {
                let application = provider.current_application().map_err(|_| ())?;
                let message = match application {
                    Some(application) => ApplicationMessage {
                        present: true,
                        application_id: application.application_id,
                        name: application.name.unwrap_or_default(),
                        executable: application.executable.unwrap_or_default(),
                    },
                    None => ApplicationMessage {
                        present: false,
                        application_id: String::new(),
                        name: String::new(),
                        executable: String::new(),
                    },
                };
                write_line(&mut stdout, &ServiceResponse::Application(message))?;
            }
            ServiceRequest::Shutdown => {
                write_line(&mut stdout, &ServiceResponse::Bye)?;
                return Ok(());
            }
        }
    }
}

fn write_line(stdout: &mut impl Write, response: &ServiceResponse) -> Result<(), ()> {
    writeln!(stdout, "{}", encode_response(response)).map_err(|_| ())?;
    stdout.flush().map_err(|_| ())
}
