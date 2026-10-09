//! Platform-neutral browser URL contract.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use wind_core::CapabilityStatus;

/// Browsers the observation contract accepts.
///
/// A value here means the executable or bundle is one of these browsers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserKind {
    Chrome,
    Chromium,
    Edge,
    Firefox,
    Safari,
}

impl BrowserKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Chromium => "chromium",
            Self::Edge => "edge",
            Self::Firefox => "firefox",
            Self::Safari => "safari",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "chrome" => Some(Self::Chrome),
            "chromium" => Some(Self::Chromium),
            "edge" => Some(Self::Edge),
            "firefox" => Some(Self::Firefox),
            "safari" => Some(Self::Safari),
            _ => None,
        }
    }
}

impl fmt::Display for BrowserKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Whether a browser integration is connected to the observation channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserIntegration {
    Unsupported,
    Unavailable,
    Connected,
    Disconnected,
}

impl BrowserIntegration {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::Unavailable => "unavailable",
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
        }
    }
}

/// One accepted URL observation. `url` is already sanitized.
#[derive(Clone, PartialEq, Eq)]
pub struct UrlObservation {
    pub browser: BrowserKind,
    pub url: String,
    pub observed_at: SystemTime,
}

impl fmt::Debug for UrlObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UrlObservation")
            .field("browser", &self.browser)
            .field("observed_at", &self.observed_at)
            .finish()
    }
}

/// Failure from a URL provider or a native message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrlError {
    Unsupported { reason: String },
    Unavailable { reason: String },
    Disconnected { reason: String },
    InvalidUrl { reason: String },
    Malformed { reason: String },
    UnsupportedBrowser { browser: String },
    QueryFailed { reason: String },
    Stopped,
}

impl fmt::Display for UrlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { reason } => {
                write!(formatter, "url tracking is unsupported: {reason}")
            }
            Self::Unavailable { reason } => {
                write!(formatter, "browser integration is unavailable: {reason}")
            }
            Self::Disconnected { reason } => {
                write!(formatter, "browser integration disconnected: {reason}")
            }
            Self::InvalidUrl { reason } => write!(formatter, "url was rejected: {reason}"),
            Self::Malformed { reason } => {
                write!(formatter, "browser message was rejected: {reason}")
            }
            Self::UnsupportedBrowser { browser } => {
                write!(formatter, "browser is not supported: {browser}")
            }
            Self::QueryFailed { reason } => write!(formatter, "url observation failed: {reason}"),
            Self::Stopped => formatter.write_str("url tracker is stopped"),
        }
    }
}

impl std::error::Error for UrlError {}

/// Reads the current browser URL. Implementations hide accessibility types.
pub trait UrlTrackerProvider: Send {
    fn capability_status(&self) -> CapabilityStatus;
    fn integration(&self) -> BrowserIntegration;
    fn current_url(&self) -> Result<Option<UrlObservation>, UrlError>;
    fn start(&mut self) -> Result<(), UrlError>;
    fn stop(&mut self) -> Result<(), UrlError>;
}

/// Scripted provider for service tests.
#[derive(Clone, Debug)]
pub enum FakeUrlOutcome {
    Url(UrlObservation),
    None,
    Unsupported,
    Unavailable,
    Disconnected,
    Idle,
    PermissionRequired,
    QueryFailed,
}

#[derive(Clone)]
pub struct FakeUrlTracker {
    outcome: Arc<Mutex<FakeUrlOutcome>>,
    started: Arc<Mutex<bool>>,
}

impl FakeUrlTracker {
    pub fn new(outcome: FakeUrlOutcome) -> Self {
        Self {
            outcome: Arc::new(Mutex::new(outcome)),
            started: Arc::new(Mutex::new(false)),
        }
    }

    pub fn set(&self, outcome: FakeUrlOutcome) {
        *self
            .outcome
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = outcome;
    }
}

impl UrlTrackerProvider for FakeUrlTracker {
    fn capability_status(&self) -> CapabilityStatus {
        let permission = matches!(
            &*self
                .outcome
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            FakeUrlOutcome::PermissionRequired
        );
        if permission {
            return CapabilityStatus::PermissionRequired;
        }
        match self.integration() {
            BrowserIntegration::Connected => CapabilityStatus::Available,
            BrowserIntegration::Unsupported => CapabilityStatus::Unsupported,
            BrowserIntegration::Unavailable | BrowserIntegration::Disconnected => {
                CapabilityStatus::Unavailable
            }
        }
    }

    fn integration(&self) -> BrowserIntegration {
        let started = *self
            .started
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !started {
            return BrowserIntegration::Unavailable;
        }
        match &*self
            .outcome
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            FakeUrlOutcome::Url(_) | FakeUrlOutcome::QueryFailed | FakeUrlOutcome::Idle => {
                BrowserIntegration::Connected
            }
            FakeUrlOutcome::Disconnected => BrowserIntegration::Disconnected,
            FakeUrlOutcome::Unsupported => BrowserIntegration::Unsupported,
            FakeUrlOutcome::PermissionRequired => BrowserIntegration::Unavailable,
            FakeUrlOutcome::None | FakeUrlOutcome::Unavailable => BrowserIntegration::Unavailable,
        }
    }

    fn current_url(&self) -> Result<Option<UrlObservation>, UrlError> {
        if !*self
            .started
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            return Err(UrlError::Stopped);
        }
        match &*self
            .outcome
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            FakeUrlOutcome::Url(observation) => Ok(Some(observation.clone())),
            FakeUrlOutcome::None | FakeUrlOutcome::Disconnected | FakeUrlOutcome::Idle => Ok(None),
            FakeUrlOutcome::PermissionRequired => Err(UrlError::Unavailable {
                reason: "accessibility permission is required".to_string(),
            }),
            FakeUrlOutcome::Unsupported => Err(UrlError::Unsupported {
                reason: "fake browser integration is unsupported".to_string(),
            }),
            FakeUrlOutcome::Unavailable => Err(UrlError::Unavailable {
                reason: "fake browser integration is unavailable".to_string(),
            }),
            FakeUrlOutcome::QueryFailed => Err(UrlError::QueryFailed {
                reason: "fake browser integration failed".to_string(),
            }),
        }
    }

    fn start(&mut self) -> Result<(), UrlError> {
        *self
            .started
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), UrlError> {
        *self
            .started
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
        Ok(())
    }
}
