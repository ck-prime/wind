//! Aggregate activity counters.
//!
//! A platform listener may see an operating-system input event. It classifies
//! that event and calls one of the `record_*` methods below. Those methods
//! take no key, character, scan code, or coordinate. The only value that
//! leaves this crate is [`ActivitySnapshot`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use wind_core::CapabilityStatus;

/// Cumulative interaction counts for one provider instance.
///
/// `snapshot` does not reset the counters. `delta_since` subtracts an earlier
/// snapshot and does not mutate either one. Addition saturates at `u64::MAX`.
#[derive(Clone, Debug)]
pub struct ActivityCounters {
    keyboard_events: Arc<AtomicU64>,
    mouse_moves: Arc<AtomicU64>,
    mouse_clicks: Arc<AtomicU64>,
    mouse_scrolls: Arc<AtomicU64>,
}

impl Default for ActivityCounters {
    fn default() -> Self {
        Self::new()
    }
}

impl ActivityCounters {
    pub fn new() -> Self {
        Self {
            keyboard_events: Arc::new(AtomicU64::new(0)),
            mouse_moves: Arc::new(AtomicU64::new(0)),
            mouse_clicks: Arc::new(AtomicU64::new(0)),
            mouse_scrolls: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Count one keyboard press. Key-up and the key identity are not recorded.
    pub fn record_keyboard_event(&self) {
        saturating_increment(&self.keyboard_events);
    }

    /// Count one pointer movement. Coordinates are not recorded.
    pub fn record_mouse_move(&self) {
        saturating_increment(&self.mouse_moves);
    }

    /// Count one button press. The button-up half is not recorded.
    pub fn record_mouse_click(&self) {
        saturating_increment(&self.mouse_clicks);
    }

    /// Count one wheel or scroll event. The wheel delta is not recorded.
    pub fn record_mouse_scroll(&self) {
        saturating_increment(&self.mouse_scrolls);
    }

    pub fn snapshot(&self) -> ActivitySnapshot {
        ActivitySnapshot {
            observed_at: SystemTime::now(),
            keyboard_events: self.keyboard_events.load(Ordering::Relaxed),
            mouse_moves: self.mouse_moves.load(Ordering::Relaxed),
            mouse_clicks: self.mouse_clicks.load(Ordering::Relaxed),
            mouse_scrolls: self.mouse_scrolls.load(Ordering::Relaxed),
        }
    }
}

fn saturating_increment(counter: &AtomicU64) {
    let mut current = counter.load(Ordering::Relaxed);
    while current < u64::MAX {
        match counter.compare_exchange_weak(
            current,
            current + 1,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return,
            Err(observed) => current = observed,
        }
    }
}

/// Point-in-time cumulative counts.
///
/// The timestamp is `SystemTime`, which is UTC. There is no local-time field
/// and no raw input field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActivitySnapshot {
    pub observed_at: SystemTime,
    pub keyboard_events: u64,
    pub mouse_moves: u64,
    pub mouse_clicks: u64,
    pub mouse_scrolls: u64,
}

impl ActivitySnapshot {
    /// Counts added between `previous` and `self`.
    ///
    /// This does not change either snapshot. A counter that went backwards
    /// saturates at zero.
    pub fn delta_since(&self, previous: &ActivitySnapshot) -> ActivitySnapshot {
        ActivitySnapshot {
            observed_at: self.observed_at,
            keyboard_events: self
                .keyboard_events
                .saturating_sub(previous.keyboard_events),
            mouse_moves: self.mouse_moves.saturating_sub(previous.mouse_moves),
            mouse_clicks: self.mouse_clicks.saturating_sub(previous.mouse_clicks),
            mouse_scrolls: self.mouse_scrolls.saturating_sub(previous.mouse_scrolls),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivityError {
    Unsupported { reason: String },
    Unavailable { reason: String },
    PermissionRequired { reason: String },
    ProviderFailed { reason: String },
    Stopped,
}

impl std::fmt::Display for ActivityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported { reason } | Self::Unavailable { reason } => {
                write!(formatter, "{reason}")
            }
            Self::PermissionRequired { reason } | Self::ProviderFailed { reason } => {
                write!(formatter, "{reason}")
            }
            Self::Stopped => write!(formatter, "activity tracker is stopped"),
        }
    }
}

impl std::error::Error for ActivityError {}

/// Platform-neutral aggregate activity source.
///
/// Implementations must not expose hook handles, event taps, or input records.
pub trait ActivityProvider: Send {
    fn start(&mut self) -> Result<(), ActivityError>;
    fn snapshot(&mut self) -> Result<ActivitySnapshot, ActivityError>;
    fn capability_status(&self) -> CapabilityStatus;
    fn stop(&mut self) -> Result<(), ActivityError>;
}

/// Deterministic provider for service tests.
#[derive(Clone, Debug)]
pub struct FakeActivityProvider {
    outcome: FakeActivityOutcome,
    counters: ActivityCounters,
    started: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FakeActivityOutcome {
    Available,
    PermissionRequired,
    Unsupported,
    Unavailable,
    Failure,
}

impl FakeActivityProvider {
    pub fn new(outcome: FakeActivityOutcome) -> Self {
        Self::with_counters(outcome, ActivityCounters::new())
    }

    pub fn with_counters(outcome: FakeActivityOutcome, counters: ActivityCounters) -> Self {
        Self {
            outcome,
            counters,
            started: false,
        }
    }

    pub fn counters(&self) -> ActivityCounters {
        self.counters.clone()
    }
}

impl ActivityProvider for FakeActivityProvider {
    fn start(&mut self) -> Result<(), ActivityError> {
        self.started = true;
        Ok(())
    }

    fn snapshot(&mut self) -> Result<ActivitySnapshot, ActivityError> {
        if !self.started {
            return Err(ActivityError::Stopped);
        }
        match self.outcome {
            FakeActivityOutcome::Available => Ok(self.counters.snapshot()),
            FakeActivityOutcome::PermissionRequired => Err(ActivityError::PermissionRequired {
                reason: "input monitoring permission is required".to_string(),
            }),
            FakeActivityOutcome::Unsupported => Err(ActivityError::Unsupported {
                reason: "input monitoring is unsupported".to_string(),
            }),
            FakeActivityOutcome::Unavailable => Err(ActivityError::Unavailable {
                reason: "input monitoring is unavailable".to_string(),
            }),
            FakeActivityOutcome::Failure => Err(ActivityError::ProviderFailed {
                reason: "activity provider failed".to_string(),
            }),
        }
    }

    fn capability_status(&self) -> CapabilityStatus {
        match self.outcome {
            FakeActivityOutcome::Available | FakeActivityOutcome::Failure => {
                CapabilityStatus::Available
            }
            FakeActivityOutcome::PermissionRequired => CapabilityStatus::PermissionRequired,
            FakeActivityOutcome::Unsupported => CapabilityStatus::Unsupported,
            FakeActivityOutcome::Unavailable => CapabilityStatus::Unavailable,
        }
    }

    fn stop(&mut self) -> Result<(), ActivityError> {
        self.started = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{ActivityCounters, ActivitySnapshot};

    #[test]
    fn counters_are_cumulative_and_do_not_reset_on_snapshot() {
        let counters = ActivityCounters::new();
        let first = counters.snapshot();
        assert_eq!(first.keyboard_events, 0);
        assert_eq!(first.mouse_moves, 0);
        assert_eq!(first.mouse_clicks, 0);
        assert_eq!(first.mouse_scrolls, 0);
        counters.record_keyboard_event();
        counters.record_keyboard_event();
        counters.record_mouse_move();
        counters.record_mouse_click();
        counters.record_mouse_scroll();
        let second = counters.snapshot();
        assert_eq!(second.keyboard_events, 2);
        assert_eq!(second.mouse_moves, 1);
        assert_eq!(second.mouse_clicks, 1);
        assert_eq!(second.mouse_scrolls, 1);
        let third = counters.snapshot();
        assert_eq!(third.keyboard_events, second.keyboard_events);
        let delta = third.delta_since(&first);
        assert_eq!(delta.keyboard_events, 2);
        assert_eq!(delta.mouse_moves, 1);
        assert_eq!(first.keyboard_events, 0);
    }

    #[test]
    fn snapshot_timestamp_is_current_utc_time() {
        let before = std::time::SystemTime::now();
        let snapshot = ActivityCounters::new().snapshot();
        let after = std::time::SystemTime::now();
        assert!(snapshot.observed_at >= before);
        assert!(snapshot.observed_at <= after);
    }

    #[test]
    fn overflow_saturates_instead_of_wrapping() {
        let counters = ActivityCounters::new();
        counters
            .keyboard_events
            .store(u64::MAX, std::sync::atomic::Ordering::Relaxed);
        counters.record_keyboard_event();
        assert_eq!(counters.snapshot().keyboard_events, u64::MAX);
    }

    #[test]
    fn public_snapshot_has_no_raw_input_fields() {
        let snapshot = ActivitySnapshot {
            observed_at: std::time::SystemTime::UNIX_EPOCH,
            keyboard_events: 1,
            mouse_moves: 2,
            mouse_clicks: 3,
            mouse_scrolls: 4,
        };
        let text = format!("{snapshot:?}");
        for forbidden in [
            "character",
            "keycode",
            "scan",
            "coordinate",
            "window",
            "url",
            "clipboard",
        ] {
            assert!(!text.to_ascii_lowercase().contains(forbidden), "{text}");
        }
    }
}
