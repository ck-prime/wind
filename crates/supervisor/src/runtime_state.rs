use std::fmt;

/// Supervisor lifetime. This is not a service lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeState {
    Initializing,
    Starting,
    Running,
    ShutdownRequested,
    Stopping,
    Stopped,
    Failed,
}

impl RuntimeState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Initializing => "Initializing",
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::ShutdownRequested => "ShutdownRequested",
            Self::Stopping => "Stopping",
            Self::Stopped => "Stopped",
            Self::Failed => "Failed",
        }
    }

    pub fn transition(self, next: Self) -> Result<Self, InvalidRuntimeTransition> {
        let allowed = matches!(
            (self, next),
            (Self::Initializing, Self::Starting)
                | (Self::Initializing, Self::ShutdownRequested)
                | (Self::Initializing, Self::Failed)
                | (Self::Starting, Self::Running)
                | (Self::Starting, Self::ShutdownRequested)
                | (Self::Starting, Self::Failed)
                | (Self::Running, Self::ShutdownRequested)
                | (Self::Running, Self::Failed)
                | (Self::ShutdownRequested, Self::Stopping)
                | (Self::Stopping, Self::Stopped)
                | (Self::Stopping, Self::Failed)
        );
        if allowed {
            Ok(next)
        } else {
            Err(InvalidRuntimeTransition {
                from: self,
                to: next,
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidRuntimeTransition {
    pub from: RuntimeState,
    pub to: RuntimeState,
}

impl fmt::Display for InvalidRuntimeTransition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid runtime transition from {} to {}",
            self.from.as_str(),
            self.to.as_str()
        )
    }
}

impl std::error::Error for InvalidRuntimeTransition {}

#[cfg(test)]
mod tests {
    use super::RuntimeState;

    #[test]
    fn resident_transitions_follow_the_shutdown_sequence() {
        let state = RuntimeState::Initializing
            .transition(RuntimeState::Starting)
            .unwrap()
            .transition(RuntimeState::Running)
            .unwrap();
        let state = state.transition(RuntimeState::ShutdownRequested).unwrap();
        let state = state.transition(RuntimeState::Stopping).unwrap();
        let state = state.transition(RuntimeState::Stopped).unwrap();
        assert_eq!(state, RuntimeState::Stopped);
    }

    #[test]
    fn invalid_runtime_transitions_are_rejected() {
        let rejected = [
            (RuntimeState::Initializing, RuntimeState::Running),
            (RuntimeState::Running, RuntimeState::Stopping),
            (RuntimeState::Running, RuntimeState::Stopped),
            (RuntimeState::ShutdownRequested, RuntimeState::Stopped),
            (RuntimeState::Stopped, RuntimeState::Running),
            (RuntimeState::Stopped, RuntimeState::ShutdownRequested),
            (RuntimeState::Failed, RuntimeState::Running),
        ];
        for (from, to) in rejected {
            let error = from.transition(to).unwrap_err();
            assert_eq!(error.from, from);
            assert_eq!(error.to, to);
            assert_eq!(from.transition(to).unwrap_err().from, from);
        }
    }
}
