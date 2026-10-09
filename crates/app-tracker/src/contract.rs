//! Platform-neutral foreground application contract.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use wind_core::CapabilityStatus;

/// One foreground application.
///
/// `application_id` is the stable identity: a bundle id, desktop id, or
/// normalized executable path. It is not a process id. `name` is the
/// application name, not a window title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveApplication {
    pub application_id: String,
    pub name: Option<String>,
    pub executable: Option<String>,
    pub observed_at: SystemTime,
}

/// Failure from an application provider. This is not a service lifecycle error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppError {
    Unsupported { reason: String },
    Unavailable { reason: String },
    QueryFailed { reason: String },
    Stopped,
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { reason } => {
                write!(formatter, "application tracking is unsupported: {reason}")
            }
            Self::Unavailable { reason } => {
                write!(formatter, "foreground application is unavailable: {reason}")
            }
            Self::QueryFailed { reason } => {
                write!(formatter, "foreground application query failed: {reason}")
            }
            Self::Stopped => formatter.write_str("application tracker is stopped"),
        }
    }
}

impl std::error::Error for AppError {}

/// Reads the foreground application. Implementations hide native window types.
pub trait AppTrackerProvider: Send + Sync {
    fn current_application(&self) -> Result<Option<ActiveApplication>, AppError>;
    fn capability_status(&self) -> CapabilityStatus;
}

/// Scripted provider for service tests.
#[derive(Clone, Debug)]
pub enum FakeAppOutcome {
    Application(ActiveApplication),
    None,
    Unsupported,
    Unavailable,
    QueryFailed,
}

#[derive(Clone)]
pub struct FakeAppTracker {
    outcome: Arc<Mutex<FakeAppOutcome>>,
}

impl FakeAppTracker {
    pub fn new(outcome: FakeAppOutcome) -> Self {
        Self {
            outcome: Arc::new(Mutex::new(outcome)),
        }
    }

    pub fn set(&self, outcome: FakeAppOutcome) {
        *self
            .outcome
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = outcome;
    }
}

impl AppTrackerProvider for FakeAppTracker {
    fn current_application(&self) -> Result<Option<ActiveApplication>, AppError> {
        match &*self
            .outcome
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            FakeAppOutcome::Application(application) => Ok(Some(application.clone())),
            FakeAppOutcome::None => Ok(None),
            FakeAppOutcome::Unsupported => Err(AppError::Unsupported {
                reason: "fake provider is unsupported".to_string(),
            }),
            FakeAppOutcome::Unavailable => Err(AppError::Unavailable {
                reason: "fake provider is unavailable".to_string(),
            }),
            FakeAppOutcome::QueryFailed => Err(AppError::QueryFailed {
                reason: "fake provider failed".to_string(),
            }),
        }
    }

    fn capability_status(&self) -> CapabilityStatus {
        match &*self
            .outcome
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            FakeAppOutcome::Unsupported => CapabilityStatus::Unsupported,
            FakeAppOutcome::Unavailable => CapabilityStatus::Unavailable,
            FakeAppOutcome::QueryFailed => CapabilityStatus::Available,
            FakeAppOutcome::Application(_) | FakeAppOutcome::None => CapabilityStatus::Available,
        }
    }
}
