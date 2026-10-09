//! Local service process.
//!
//! The supervisor spawns a validated executable and speaks the service protocol
//! on inherited stdin and stdout. Those pipes exist only for this parent and
//! child. There is no listening socket and no network address. Stderr stays
//! the log stream. The child does not receive secret arguments.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use wind_core::{
    decode_response, encode_request, ApplicationMessage, CapabilityStatus, Error, HealthStatus,
    ProcessState, ServiceId, ServiceRequest, ServiceResponse, Version, SERVICE_PROTOCOL_VERSION,
};

use wind_platform::service_executable_name;

/// A process is restarted at most this many times after an unexpected exit.
pub const MAX_SERVICE_RESTARTS: u32 = 3;

const STOP_BUDGET: Duration = Duration::from_secs(2);
const RESTART_BACKOFF: Duration = Duration::from_millis(50);

pub struct LaunchSpec {
    pub service_id: ServiceId,
    pub version: Version,
    pub executable: PathBuf,
    /// Debug stand-in selector. Production launches leave this empty.
    pub test_mode: Option<String>,
}

pub struct ServiceProcess {
    spec: LaunchSpec,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    state: ProcessState,
    restarts: u32,
}

impl ServiceProcess {
    pub fn launch(spec: LaunchSpec) -> Result<Self, Error> {
        validate_executable(spec.service_id, &spec.executable)?;
        let mut started = spawn(&spec)?;
        match handshake(&mut started, &spec) {
            Ok(()) => Ok(started),
            Err(error) => {
                started.kill_now();
                Err(error)
            }
        }
    }

    pub fn state(&self) -> ProcessState {
        self.state
    }

    pub fn restarts(&self) -> u32 {
        self.restarts
    }

    pub fn is_running(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,
            _ => {
                if self.state == ProcessState::Running {
                    self.state = ProcessState::Crashed;
                }
                false
            }
        }
    }

    pub fn health(&mut self) -> Result<(HealthStatus, CapabilityStatus), Error> {
        if !self.is_running() {
            return Err(process_error(
                self.spec.service_id,
                "service process is not running",
            ));
        }
        self.request(&ServiceRequest::Health)?;
        match self.read_response()? {
            ServiceResponse::Health(health) => Ok((health.status, health.capability)),
            ServiceResponse::Error(error) => {
                Err(process_error(self.spec.service_id, error.explain()))
            }
            _ => Err(process_error(
                self.spec.service_id,
                "service returned an unexpected health response",
            )),
        }
    }

    pub fn current_application(&mut self) -> Result<Option<ApplicationMessage>, Error> {
        if !self.is_running() {
            return Err(process_error(
                self.spec.service_id,
                "service process is not running",
            ));
        }
        self.request(&ServiceRequest::CurrentApplication)?;
        match self.read_response()? {
            ServiceResponse::Application(application) => Ok(Some(application)),
            ServiceResponse::Error(error) => {
                Err(process_error(self.spec.service_id, error.explain()))
            }
            _ => Err(process_error(
                self.spec.service_id,
                "service returned an unexpected application response",
            )),
        }
    }

    /// Restart a crashed process until the attempt limit.
    pub fn recover(&mut self) -> Result<(), Error> {
        if self.is_running() {
            return Ok(());
        }
        self.restarts += 1;
        if self.restarts > MAX_SERVICE_RESTARTS {
            self.state = ProcessState::Crashed;
            return Err(process_error(
                self.spec.service_id,
                format!("service crash loop after {} restarts", MAX_SERVICE_RESTARTS),
            ));
        }
        self.state = ProcessState::Crashed;
        thread::sleep(RESTART_BACKOFF.saturating_mul(self.restarts));
        self.kill_now();
        let mut started = spawn(&self.spec)?;
        if let Err(error) = handshake(&mut started, &self.spec) {
            started.kill_now();
            return Err(error);
        }
        started.restarts = self.restarts;
        *self = started;
        Ok(())
    }

    pub fn shutdown(&mut self) {
        if !self.is_running() {
            self.state = ProcessState::Stopped;
            return;
        }
        self.state = ProcessState::Stopping;
        let _ = self.request(&ServiceRequest::Shutdown);
        let deadline = Instant::now() + STOP_BUDGET;
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                self.state = ProcessState::Stopped;
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        self.kill_now();
        self.state = ProcessState::Stopped;
    }

    fn request(&mut self, request: &ServiceRequest) -> Result<(), Error> {
        let line = encode_request(request);
        writeln!(self.stdin, "{line}").map_err(|_| {
            process_error(self.spec.service_id, "service pipe could not be written")
        })?;
        self.stdin.flush().map_err(|_| {
            process_error(self.spec.service_id, "service pipe could not be written")
        })?;
        Ok(())
    }

    fn read_response(&mut self) -> Result<ServiceResponse, Error> {
        let mut line = String::new();
        let read = self
            .stdout
            .read_line(&mut line)
            .map_err(|_| process_error(self.spec.service_id, "service pipe could not be read"))?;
        if read == 0 {
            self.state = ProcessState::Crashed;
            return Err(process_error(
                self.spec.service_id,
                "service process closed the pipe",
            ));
        }
        decode_response(&line).map_err(|error| process_error(self.spec.service_id, error.explain()))
    }

    fn kill_now(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.state = ProcessState::Stopped;
    }
}

impl Drop for ServiceProcess {
    fn drop(&mut self) {
        if self.is_running() {
            self.kill_now();
        }
    }
}

fn spawn(spec: &LaunchSpec) -> Result<ServiceProcess, Error> {
    let mut command = Command::new(&spec.executable);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .env_remove("WIND_SERVICE_TEST_MODE")
        .env_remove("WIND_SERVICE_TEST_ID")
        .env_remove("WIND_SERVICE_TEST_VERSION");
    if let Some(mode) = &spec.test_mode {
        command
            .env("WIND_SERVICE_TEST_MODE", mode)
            .env("WIND_SERVICE_TEST_ID", spec.service_id.as_str())
            .env("WIND_SERVICE_TEST_VERSION", spec.version.to_string());
    }
    let mut child = command
        .spawn()
        .map_err(|_| process_error(spec.service_id, "service executable could not be started"))?;
    let stdin = child.stdin.take().ok_or_else(|| {
        let _ = child.kill();
        process_error(spec.service_id, "service stdin was not available")
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        let _ = child.kill();
        process_error(spec.service_id, "service stdout was not available")
    })?;
    Ok(ServiceProcess {
        spec: LaunchSpec {
            service_id: spec.service_id,
            version: spec.version,
            executable: spec.executable.clone(),
            test_mode: spec.test_mode.clone(),
        },
        child,
        stdin,
        stdout: BufReader::new(stdout),
        state: ProcessState::Starting,
        restarts: 0,
    })
}

fn handshake(process: &mut ServiceProcess, spec: &LaunchSpec) -> Result<(), Error> {
    process.request(&ServiceRequest::Hello)?;
    match process.read_response()? {
        ServiceResponse::Ready(ready) => {
            if ready.service_id != spec.service_id {
                return Err(process_error(
                    spec.service_id,
                    format!(
                        "service identity {} was rejected",
                        ready.service_id.as_str()
                    ),
                ));
            }
            if ready.protocol_version != SERVICE_PROTOCOL_VERSION {
                return Err(process_error(
                    spec.service_id,
                    wind_core::ProtocolError::incompatible(
                        SERVICE_PROTOCOL_VERSION,
                        ready.protocol_version,
                    )
                    .explain(),
                ));
            }
            if ready.version != spec.version {
                return Err(process_error(
                    spec.service_id,
                    format!(
                        "service version {} does not match installed version {}",
                        ready.version, spec.version
                    ),
                ));
            }
            process.state = ProcessState::Running;
            Ok(())
        }
        ServiceResponse::Error(error) => Err(process_error(spec.service_id, error.explain())),
        _ => Err(process_error(
            spec.service_id,
            "service did not send metadata",
        )),
    }
}

pub fn validate_executable(id: ServiceId, path: &Path) -> Result<(), Error> {
    if !path.is_absolute() {
        return Err(process_error(
            id,
            "service executable path must be absolute",
        ));
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| process_error(id, "service executable could not be found"))?;
    let expected = service_executable_name(id);
    let actual = canonical.file_name().and_then(|name| name.to_str());
    if actual != Some(expected.as_str()) {
        return Err(process_error(
            id,
            format!("service executable name must be {expected}"),
        ));
    }
    Ok(())
}

fn process_error(id: ServiceId, message: impl Into<String>) -> Error {
    Error::new("supervisor", "service_process", message).with_service(id.as_str())
}
