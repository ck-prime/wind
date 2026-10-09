//! Platform facts used by the supervisor.
//!
//! Operating-system queries stay in this crate. Business logic should consume
//! [`PlatformInfo`], [`SessionContext`], and [`RuntimePaths`] instead of
//! checking the target OS itself.
//!
//! This crate detects the host, the current user session, and user-scoped
//! runtime directories. Screenshot, video, foreground application, and browser
//! URL providers are dispatched to the platform crate for this operating
//! system. Aggregate activity counters are dispatched the same way. Raw input
//! events stay inside the platform provider. This crate does not install a
//! system service.

mod activity;
mod app_tracker;
mod paths;
mod permissions;
mod screenshot;
mod secret;
mod signals;
mod url_tracker;
mod video;

use std::env;
use std::fmt;
use std::process::Command;

pub use activity::host_activity_provider;
pub use app_tracker::{host_app_tracker, HostAppTracker};
pub use paths::{
    ensure_runtime_paths, host_runtime_paths, linux_runtime_paths, macos_runtime_paths,
    windows_runtime_paths, LinuxPathEnv, PlatformError, RuntimePaths,
};
pub use permissions::{
    apply_session, guide_permission_until_granted, guide_permissions_until_granted,
    linux_permission, macos_permission, windows_permission, HostPermissionProvider,
    PermissionGuideChoice,
};
pub use screenshot::{host_screenshot_provider, HostScreenshotProvider};
pub use secret::host_secret_store;
pub use signals::install_termination_handler;

/// Block until `is_requested` or `timeout`.
///
/// On macOS the main run loop is pumped so a Quit & Reopen request can land.
/// `park` is the wait used on other systems.
pub fn wait_until_quit(
    timeout: std::time::Duration,
    is_requested: impl Fn() -> bool,
    park: impl Fn(std::time::Duration) -> bool,
) -> bool {
    #[cfg(target_os = "macos")]
    {
        let _ = park;
        let started = std::time::Instant::now();
        while !is_requested() {
            let left = timeout.saturating_sub(started.elapsed());
            if left.is_zero() {
                return false;
            }
            wind_macos::pump_main_run_loop(left.min(std::time::Duration::from_millis(200)));
        }
        true
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = is_requested;
        park(timeout)
    }
}

/// Accept macOS Quit & Reopen. Other systems ignore this.
pub fn install_system_quit_handler(notify: impl Fn() + Send + Sync + 'static) {
    #[cfg(target_os = "macos")]
    {
        wind_macos::install_system_quit_handler(notify);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = notify;
    }
}

/// Close a visible permission dialog. Other systems do nothing.
pub fn dismiss_permission_guide() {
    #[cfg(target_os = "macos")]
    wind_macos::dismiss_permission_guide();
}

/// Finish a Quit & Reopen that Wind deferred until its own shutdown completed.
pub fn finish_system_quit() {
    #[cfg(target_os = "macos")]
    wind_macos::finish_system_quit();
}
pub use url_tracker::{host_url_tracker, HostUrlTracker};
pub use video::{host_video_provider, HostVideoProvider};

/// File name of a service executable on this operating system.
///
/// The name carries the stable service id. It does not carry the service version
/// or the product display name.
pub fn service_executable_name(id: wind_core::ServiceId) -> String {
    let stem = format!("wind-{}", id.technical_component());
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem
    }
}

/// Host facts available without requesting privacy permissions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformInfo {
    pub operating_system: OperatingSystem,
    pub architecture: Architecture,
    pub platform_version: String,
    pub user_session: UserSession,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatingSystem {
    MacOs,
    Windows,
    Linux,
    Other(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    X86_64,
    Aarch64,
    Other(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Interactive,
    NonInteractive,
    Unknown,
}

/// Best-effort description of the current user session.
///
/// Values come from the process environment. This does not query privilege
/// or privacy databases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserSession {
    pub user: Option<String>,
    pub kind: SessionKind,
}

impl OperatingSystem {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MacOs => "macos",
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::Other(name) => name,
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::MacOs => "macOS",
            Self::Windows => "Windows",
            Self::Linux => "Linux",
            Self::Other(name) => name,
        }
    }

    fn from_std_name(name: &'static str) -> Self {
        match name {
            "macos" => Self::MacOs,
            "windows" => Self::Windows,
            "linux" => Self::Linux,
            other => Self::Other(other),
        }
    }
}

impl Architecture {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
            Self::Other(name) => name,
        }
    }

    fn from_std_name(name: &'static str) -> Self {
        match name {
            "x86_64" => Self::X86_64,
            "aarch64" => Self::Aarch64,
            other => Self::Other(other),
        }
    }
}

impl fmt::Display for SessionKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Interactive => "Interactive",
            Self::NonInteractive => "NonInteractive",
            Self::Unknown => "Unknown",
        })
    }
}

impl UserSession {
    pub fn describe(&self) -> String {
        match &self.user {
            Some(user) => format!("{}:{user}", self.kind),
            None => self.kind.to_string(),
        }
    }
}

/// Current user, limited to a name and an optional numeric id.
///
/// This does not include email, machine identifiers, files, credentials, or
/// environment secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserContext {
    pub username: Option<String>,
    pub user_id: Option<String>,
}

/// Session facts a future capability can read without querying the OS itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionContext {
    pub kind: SessionKind,
    pub user: UserContext,
}

/// Host runtime facts. Implementations must not expose OS-specific types.
pub trait PlatformRuntime {
    fn platform_info(&self) -> PlatformInfo;

    fn current_user(&self) -> Result<UserContext, PlatformError>;

    fn session_context(&self) -> Result<SessionContext, PlatformError>;

    fn runtime_paths(&self, product_identifier: &str) -> Result<RuntimePaths, PlatformError>;
}

/// Runtime backed by the process environment and this target's path layout.
#[derive(Debug, Default, Clone, Copy)]
pub struct HostRuntime;

impl PlatformRuntime for HostRuntime {
    fn platform_info(&self) -> PlatformInfo {
        detect()
    }

    fn current_user(&self) -> Result<UserContext, PlatformError> {
        Ok(current_user())
    }

    fn session_context(&self) -> Result<SessionContext, PlatformError> {
        Ok(session_context())
    }

    fn runtime_paths(&self, product_identifier: &str) -> Result<RuntimePaths, PlatformError> {
        host_runtime_paths(product_identifier)
    }
}

pub fn host_runtime() -> HostRuntime {
    #[cfg(target_os = "macos")]
    wind_macos::prepare_background_application();
    HostRuntime
}

/// Classify a session without reading the operating system.
pub fn classify_session(has_user: bool, interactive: bool) -> SessionKind {
    if !has_user {
        SessionKind::Unknown
    } else if interactive {
        SessionKind::Interactive
    } else {
        SessionKind::NonInteractive
    }
}

impl PlatformInfo {
    pub fn describe(&self) -> String {
        format!(
            "{} {} ({})",
            self.operating_system.display_name(),
            self.architecture.as_str(),
            self.platform_version
        )
    }
}

/// Detect the operating system, architecture, version, and user session.
pub fn detect() -> PlatformInfo {
    let operating_system = OperatingSystem::from_std_name(env::consts::OS);
    let architecture = Architecture::from_std_name(env::consts::ARCH);
    PlatformInfo {
        platform_version: platform_version(operating_system),
        user_session: user_session(),
        operating_system,
        architecture,
    }
}

fn platform_version(operating_system: OperatingSystem) -> String {
    let version = match operating_system {
        OperatingSystem::MacOs => macos_version(),
        OperatingSystem::Windows => windows_version(),
        OperatingSystem::Linux => linux_version(),
        OperatingSystem::Other(_) => None,
    };
    version.unwrap_or_else(|| "unknown".to_string())
}

fn macos_version() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        read_command_line("sw_vers", &["-productVersion"])
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

fn windows_version() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        read_command_line("cmd", &["/C", "ver"])
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

fn linux_version() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        read_os_release()
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn read_command_line(program: &str, args: &[&str]) -> Option<String> {
    let mut command = Command::new(program);
    command.args(args);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let bytes = if output.stdout.iter().any(|byte| !byte.is_ascii_whitespace()) {
        &output.stdout
    } else {
        &output.stderr
    };
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToString::to_string)
}

#[cfg(target_os = "linux")]
fn read_os_release() -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    let mut pretty = None;
    let mut name = None;
    let mut version = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"').to_string();
        match key {
            "PRETTY_NAME" => pretty = Some(value),
            "NAME" => name = Some(value),
            "VERSION_ID" => version = Some(value),
            _ => {}
        }
    }
    pretty.or(match (name, version) {
        (Some(name), Some(version)) => Some(format!("{name} {version}")),
        (Some(name), None) => Some(name),
        _ => None,
    })
}

fn username() -> Option<String> {
    env::var("USERNAME")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| env::var("USER").ok().filter(|value| !value.is_empty()))
}

fn user_session() -> UserSession {
    let user = username();
    let kind = classify_session(user.is_some(), interactive_environment());
    UserSession { user, kind }
}

fn current_user() -> UserContext {
    UserContext {
        username: username(),
        user_id: current_user_id(),
    }
}

fn session_context() -> SessionContext {
    let user = current_user();
    let kind = classify_session(user.username.is_some(), interactive_environment());
    SessionContext { kind, user }
}

fn current_user_id() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        linux_user_id()
    }
    #[cfg(target_os = "macos")]
    {
        read_command_line("id", &["-u"])
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
fn linux_user_id() -> Option<String> {
    let text = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            return rest.split_whitespace().next().map(str::to_string);
        }
    }
    None
}

fn interactive_environment() -> bool {
    match env::consts::OS {
        "windows" | "macos" => true,
        "linux" => {
            env::var_os("DISPLAY").is_some()
                || env::var_os("WAYLAND_DISPLAY").is_some()
                || env::var("XDG_SESSION_TYPE")
                    .ok()
                    .is_some_and(|value| !value.is_empty() && value != "tty")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        classify_session, detect, host_runtime, Architecture, OperatingSystem, PlatformRuntime,
        SessionKind,
    };

    #[test]
    fn detects_operating_system() {
        let info = detect();
        assert_eq!(
            info.operating_system,
            OperatingSystem::from_std_name(std::env::consts::OS)
        );
        assert!(!info.operating_system.display_name().is_empty());
        assert!(!info.platform_version.is_empty());
        assert!(!info.user_session.describe().is_empty());
    }

    #[test]
    fn detects_architecture() {
        let info = detect();
        assert_eq!(info.architecture.as_str(), std::env::consts::ARCH);
        assert_eq!(
            info.architecture,
            Architecture::from_std_name(std::env::consts::ARCH)
        );
    }

    #[test]
    fn session_classification_is_platform_neutral() {
        assert_eq!(classify_session(true, true), SessionKind::Interactive);
        assert_eq!(classify_session(true, false), SessionKind::NonInteractive);
        assert_eq!(classify_session(false, true), SessionKind::Unknown);
        assert_eq!(classify_session(false, false), SessionKind::Unknown);
        assert_eq!(SessionKind::Interactive.to_string(), "Interactive");
        assert_eq!(SessionKind::NonInteractive.to_string(), "NonInteractive");
        assert_eq!(SessionKind::Unknown.to_string(), "Unknown");
    }

    #[test]
    fn host_session_matches_the_current_user() {
        let runtime = host_runtime();
        let user = runtime.current_user().unwrap();
        let session = runtime.session_context().unwrap();
        assert_eq!(session.user, user);
        if user.username.is_some() {
            assert_ne!(session.kind, SessionKind::Unknown);
        } else {
            assert_eq!(session.kind, SessionKind::Unknown);
        }
        #[cfg(target_os = "windows")]
        assert!(user.user_id.is_none());
    }
}
