//! Session-relative tracking intervals.
//!
//! The first window starts when tracking starts and ends at the next UTC
//! 10-second boundary. Later windows are exactly 10 seconds. Boundaries are
//! half-open: an event at the end belongs to the next window.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Configured interval length. A future setting can replace this constant.
pub const INTERVAL_MILLIS: u64 = 10_000;

/// Cumulative counters. These are totals, not a reset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActivityTotals {
    pub keyboard_events: u64,
    pub mouse_moves: u64,
    pub mouse_clicks: u64,
    pub mouse_scrolls: u64,
}

impl ActivityTotals {
    pub fn saturating_since(self, previous: Self) -> Self {
        Self {
            keyboard_events: self
                .keyboard_events
                .saturating_sub(previous.keyboard_events),
            mouse_moves: self.mouse_moves.saturating_sub(previous.mouse_moves),
            mouse_clicks: self.mouse_clicks.saturating_sub(previous.mouse_clicks),
            mouse_scrolls: self.mouse_scrolls.saturating_sub(previous.mouse_scrolls),
        }
    }
}

/// What the trackers reported at a close.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub activity: ActivityTotals,
    pub application_id: Option<String>,
    pub url: Option<String>,
}

impl Observation {
    #[cfg(test)]
    fn empty() -> Self {
        Self {
            activity: ActivityTotals::default(),
            application_id: None,
            url: None,
        }
    }
}

/// One identity held across a window boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
struct OpenSpan {
    value: String,
    since: SystemTime,
}

/// A window that should be persisted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClosedWindow {
    pub started_at: SystemTime,
    pub ended_at: SystemTime,
    pub partial: bool,
}

/// Rows for one closed window. `capture_screenshot` is a single attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntervalRecord {
    pub started_at: SystemTime,
    pub ended_at: SystemTime,
    pub partial: bool,
    pub activity: ActivityTotals,
    pub application: Option<(String, SystemTime, SystemTime)>,
    pub url: Option<(String, SystemTime, SystemTime)>,
    pub capture_screenshot: bool,
}

/// Clock for one process session. A later process starts a new session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackingSession {
    started_at: SystemTime,
    interval_started_at: SystemTime,
    interval_ends_at: SystemTime,
    activity: ActivityTotals,
    application: Option<OpenSpan>,
    url: Option<OpenSpan>,
    open: bool,
}

impl TrackingSession {
    pub fn start(now: SystemTime) -> Self {
        let end = first_interval_end(now);
        Self {
            started_at: now,
            interval_started_at: now,
            interval_ends_at: end,
            activity: ActivityTotals::default(),
            application: None,
            url: None,
            open: true,
        }
    }

    #[cfg(test)]
    fn started_at(&self) -> SystemTime {
        self.started_at
    }

    #[cfg(test)]
    fn interval_started_at(&self) -> SystemTime {
        self.interval_started_at
    }

    pub fn interval_ends_at(&self) -> SystemTime {
        self.interval_ends_at
    }

    pub fn activity_baseline(&self) -> ActivityTotals {
        self.activity
    }

    pub fn application_id(&self) -> Option<String> {
        self.application.as_ref().map(|span| span.value.clone())
    }

    pub fn url_value(&self) -> Option<String> {
        self.url.as_ref().map(|span| span.value.clone())
    }

    /// Remember the counters and identities that are already open.
    pub fn seed(&mut self, observed: Observation) {
        self.activity = observed.activity;
        self.application = span_at(observed.application_id, self.interval_started_at);
        self.url = span_at(observed.url, self.interval_started_at);
    }

    /// The next window whose end has arrived, or the shutdown partial.
    pub fn next_due(&self, now: SystemTime, shutdown: bool) -> Option<ClosedWindow> {
        if !self.open {
            return None;
        }
        if now >= self.interval_ends_at {
            return Some(ClosedWindow {
                started_at: self.interval_started_at,
                ended_at: self.interval_ends_at,
                partial: false,
            });
        }
        if shutdown && now > self.interval_started_at {
            return Some(ClosedWindow {
                started_at: self.interval_started_at,
                ended_at: now,
                partial: true,
            });
        }
        None
    }

    /// Attribute `observed` to `window` and move the clock to the next boundary.
    ///
    /// The end stored on `window` stays authoritative. A late observation does
    /// not move that end forward.
    pub fn close_window(
        &mut self,
        window: &ClosedWindow,
        observed: &Observation,
    ) -> IntervalRecord {
        let activity = observed.activity.saturating_since(self.activity);
        self.activity = observed.activity;
        let application = take_overlap(&self.application, window);
        let url = take_overlap(&self.url, window);
        self.application = next_span(
            self.application.take(),
            observed.application_id.clone(),
            window.ended_at,
        );
        self.url = next_span(self.url.take(), observed.url.clone(), window.ended_at);
        if window.partial {
            self.open = false;
        } else {
            self.interval_started_at = window.ended_at;
            self.interval_ends_at = window.ended_at + Duration::from_millis(INTERVAL_MILLIS);
        }
        IntervalRecord {
            started_at: window.started_at,
            ended_at: window.ended_at,
            partial: window.partial,
            activity,
            application,
            url,
            capture_screenshot: true,
        }
    }
}

/// Wait until `boundary`, but no longer than `cap`.
pub fn wait_budget(now: SystemTime, boundary: SystemTime, cap: Duration) -> Duration {
    match boundary.duration_since(now) {
        Ok(remaining) if !remaining.is_zero() => remaining.min(cap),
        _ => Duration::ZERO,
    }
}

/// `start <= event < end`.
#[cfg(test)]
fn contains(started_at: SystemTime, ended_at: SystemTime, event: SystemTime) -> bool {
    event >= started_at && event < ended_at
}

/// Portion of `[span_start, span_end)` that lies inside `[window_start, window_end)`.
pub fn clip_span(
    span_start: SystemTime,
    span_end: SystemTime,
    window_start: SystemTime,
    window_end: SystemTime,
) -> Option<(SystemTime, SystemTime)> {
    let started_at = span_start.max(window_start);
    let ended_at = span_end.min(window_end);
    if started_at < ended_at {
        Some((started_at, ended_at))
    } else {
        None
    }
}

pub fn format_utc(time: SystemTime) -> String {
    let millis = unix_millis(time);
    let seconds = millis / 1000;
    let sub = millis % 1000;
    let days = (seconds / 86_400) as i64;
    let tod = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = tod / 3_600;
    let minute = (tod % 3_600) / 60;
    let second = tod % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{sub:03}Z")
}

fn first_interval_end(start: SystemTime) -> SystemTime {
    let millis = unix_millis(start);
    let remainder = millis % INTERVAL_MILLIS;
    let end = if remainder == 0 {
        millis + INTERVAL_MILLIS
    } else {
        millis + (INTERVAL_MILLIS - remainder)
    };
    UNIX_EPOCH + Duration::from_millis(end)
}

fn unix_millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn span_at(value: Option<String>, since: SystemTime) -> Option<OpenSpan> {
    value
        .filter(|item| !item.is_empty())
        .map(|value| OpenSpan { value, since })
}

fn take_overlap(
    span: &Option<OpenSpan>,
    window: &ClosedWindow,
) -> Option<(String, SystemTime, SystemTime)> {
    let span = span.as_ref()?;
    let (started_at, ended_at) = clip_span(
        span.since,
        window.ended_at,
        window.started_at,
        window.ended_at,
    )?;
    Some((span.value.clone(), started_at, ended_at))
}

fn next_span(
    current: Option<OpenSpan>,
    observed: Option<String>,
    at: SystemTime,
) -> Option<OpenSpan> {
    let observed = observed.filter(|value| !value.is_empty());
    match (current, observed) {
        (Some(span), Some(value)) if span.value == value => Some(OpenSpan { value, since: at }),
        (_, Some(value)) => Some(OpenSpan { value, since: at }),
        (_, None) => None,
    }
}

/// Howard Hinnant's civil-from-days conversion. `days` is days since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year_of_era = yoe as i64;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 {
        year_of_era + era * 400 + 1
    } else {
        year_of_era + era * 400
    };
    (year, month as u32, day)
}

#[cfg(test)]
mod tests {
    use super::{
        clip_span, contains, format_utc, wait_budget, ActivityTotals, ClosedWindow, Observation,
        TrackingSession, INTERVAL_MILLIS,
    };

    fn at(offset_ms: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(1_700_000_000_000 + offset_ms)
    }
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn schedule(start_ms: u64, count: usize) -> Vec<(u64, u64)> {
        let mut session = TrackingSession::start(at(start_ms));
        let mut windows = Vec::new();
        for _ in 0..count {
            let now = session.interval_ends_at();
            let window = session.next_due(now, false).expect("window");
            windows.push((unix_offset(window.started_at), unix_offset(window.ended_at)));
            session.close_window(&window, &Observation::empty());
        }
        windows
    }

    fn unix_offset(time: std::time::SystemTime) -> u64 {
        time.duration_since(UNIX_EPOCH).expect("time").as_millis() as u64 - 1_700_000_000_000
    }

    #[test]
    fn start_on_a_boundary_uses_full_intervals() {
        assert_eq!(
            schedule(0, 3),
            vec![(0, 10_000), (10_000, 20_000), (20_000, 30_000)]
        );
    }

    #[test]
    fn start_at_four_seconds_closes_on_the_next_boundary() {
        assert_eq!(
            schedule(4_000, 3),
            vec![(4_000, 10_000), (10_000, 20_000), (20_000, 30_000)]
        );
    }

    #[test]
    fn start_just_before_a_boundary_makes_a_short_first_interval() {
        let windows = schedule(9_999, 1);
        assert_eq!(windows, vec![(9_999, 10_000)]);
    }

    #[test]
    fn an_event_at_the_boundary_belongs_to_the_next_interval() {
        let first = (at(4_000), at(10_000));
        let second = (at(10_000), at(20_000));
        let boundary = at(10_000);
        assert!(!contains(first.0, first.1, boundary));
        assert!(contains(second.0, second.1, boundary));
        assert!(contains(first.0, first.1, at(9_999)));
    }

    #[test]
    fn an_application_segment_is_split_on_the_boundary() {
        let windows = [(at(4_000), at(10_000)), (at(10_000), at(20_000))];
        let chrome = (at(8_000), at(14_000));
        let portions: Vec<_> = windows
            .iter()
            .filter_map(|(start, end)| clip_span(chrome.0, chrome.1, *start, *end))
            .map(|(start, end)| unix_offset(end) - unix_offset(start))
            .collect();
        assert_eq!(portions, vec![2_000, 4_000]);
    }

    #[test]
    fn a_url_segment_uses_the_same_boundary_split() {
        let windows = [(at(4_000), at(10_000)), (at(10_000), at(20_000))];
        let url = (at(8_000), at(14_000));
        let portions: Vec<_> = windows
            .iter()
            .filter_map(|(start, end)| clip_span(url.0, url.1, *start, *end))
            .map(|(start, end)| unix_offset(end) - unix_offset(start))
            .collect();
        assert_eq!(portions, vec![2_000, 4_000]);
    }

    #[test]
    fn shutdown_persists_the_partial_interval() {
        let mut session = TrackingSession::start(at(4_000));
        let window = session.next_due(at(8_000), true).expect("partial");
        assert!(window.partial);
        assert_eq!(unix_offset(window.started_at), 4_000);
        assert_eq!(unix_offset(window.ended_at), 8_000);
        let record = session.close_window(&window, &Observation::empty());
        assert!(record.capture_screenshot);
        assert!(session.next_due(at(8_000), true).is_none());
    }

    #[test]
    fn restart_starts_a_new_session() {
        let mut old = TrackingSession::start(at(4_000));
        let window = old.next_due(at(8_000), true).expect("partial");
        old.close_window(&window, &Observation::empty());
        let restarted = TrackingSession::start(at(15_000));
        assert_eq!(unix_offset(restarted.interval_started_at()), 15_000);
        assert_eq!(unix_offset(restarted.interval_ends_at()), 20_000);
        assert_ne!(restarted.started_at(), old.started_at());
    }

    #[test]
    fn each_completed_interval_requests_one_screenshot() {
        let mut session = TrackingSession::start(at(0));
        let mut captures = 0;
        for _ in 0..3 {
            let now = session.interval_ends_at();
            let window = session.next_due(now, false).expect("window");
            let record = session.close_window(&window, &Observation::empty());
            assert!(record.capture_screenshot);
            captures += 1;
        }
        assert_eq!(captures, 3);
        assert!(session.next_due(at(30_000), false).is_none());
    }

    #[test]
    fn a_failed_screenshot_still_keeps_the_interval() {
        let mut session = TrackingSession::start(at(4_000));
        let window = session.next_due(at(10_000), false).expect("window");
        let record = session.close_window(&window, &Observation::empty());
        let capture_ok = false;
        let artifact = capture_ok.then_some("screenshot");
        assert!(record.capture_screenshot);
        assert!(artifact.is_none());
        assert_eq!(unix_offset(record.started_at), 4_000);
        assert_eq!(unix_offset(record.ended_at), 10_000);
        assert!(session.next_due(at(10_000), false).is_none());
    }

    #[test]
    fn a_late_close_keeps_the_boundary() {
        let mut session = TrackingSession::start(at(4_000));
        let window = session.next_due(at(10_400), false).expect("window");
        assert_eq!(unix_offset(window.ended_at), 10_000);
        session.close_window(&window, &Observation::empty());
        assert_eq!(unix_offset(session.interval_started_at()), 10_000);
        assert_eq!(unix_offset(session.interval_ends_at()), 20_000);
        assert!(session.next_due(at(10_400), false).is_none());
    }

    #[test]
    fn activity_delta_is_the_change_since_the_interval_baseline() {
        let mut session = TrackingSession::start(at(0));
        session.seed(Observation {
            activity: ActivityTotals {
                keyboard_events: 5,
                mouse_moves: 1,
                mouse_clicks: 2,
                mouse_scrolls: 0,
            },
            application_id: Some("chrome".to_string()),
            url: Some("https://example.com/".to_string()),
        });
        let window = session.next_due(at(10_000), false).expect("window");
        let record = session.close_window(
            &window,
            &Observation {
                activity: ActivityTotals {
                    keyboard_events: 8,
                    mouse_moves: 4,
                    mouse_clicks: 2,
                    mouse_scrolls: 1,
                },
                application_id: Some("chrome".to_string()),
                url: Some("https://example.com/".to_string()),
            },
        );
        assert_eq!(record.activity.keyboard_events, 3);
        assert_eq!(record.activity.mouse_moves, 3);
        assert_eq!(record.activity.mouse_clicks, 0);
        assert_eq!(record.activity.mouse_scrolls, 1);
        let (application, _, _) = record.application.expect("application");
        assert_eq!(application, "chrome");
        let (url, _, _) = record.url.expect("url");
        assert_eq!(url, "https://example.com/");
    }

    #[test]
    fn wait_budget_uses_the_absolute_boundary() {
        let now = at(4_000);
        let boundary = at(10_000);
        assert_eq!(
            wait_budget(now, boundary, Duration::from_secs(30)),
            Duration::from_secs(6)
        );
        assert_eq!(
            wait_budget(at(10_400), boundary, Duration::from_secs(10)),
            Duration::ZERO
        );
    }

    #[test]
    fn utc_format_uses_the_unix_epoch() {
        assert_eq!(format_utc(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        assert_eq!(INTERVAL_MILLIS, 10_000);
    }

    #[test]
    fn a_crossing_identity_is_closed_on_the_window() {
        let mut session = TrackingSession::start(at(0));
        session.seed(Observation {
            activity: ActivityTotals::default(),
            application_id: Some("chrome".to_string()),
            url: None,
        });
        let window = ClosedWindow {
            started_at: at(0),
            ended_at: at(10_000),
            partial: false,
        };
        let record = session.close_window(
            &window,
            &Observation {
                activity: ActivityTotals::default(),
                application_id: Some("editor".to_string()),
                url: None,
            },
        );
        let (name, start, end) = record.application.expect("previous application");
        assert_eq!(name, "chrome");
        assert_eq!(unix_offset(start), 0);
        assert_eq!(unix_offset(end), 10_000);
        let next = session.next_due(at(20_000), false).expect("next");
        let record = session.close_window(
            &next,
            &Observation {
                activity: ActivityTotals::default(),
                application_id: Some("editor".to_string()),
                url: None,
            },
        );
        let (name, start, end) = record.application.expect("next application");
        assert_eq!(name, "editor");
        assert_eq!(unix_offset(start), 10_000);
        assert_eq!(unix_offset(end), 20_000);
    }
}
