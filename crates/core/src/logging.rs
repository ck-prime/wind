#[cfg(not(debug_assertions))]
use std::fs::OpenOptions;
#[cfg(debug_assertions)]
use std::io;
use std::io::Write;
#[cfg(not(debug_assertions))]
use std::path::PathBuf;
#[cfg(not(debug_assertions))]
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
        }
    }
}

/// Fields every component can attach to a local log event.
pub struct LogFields<'a> {
    pub level: Level,
    pub component: &'a str,
    pub service_id: Option<&'a str>,
    pub version: Option<&'a str>,
    pub platform: Option<&'a str>,
    pub message: &'a str,
    pub error: Option<&'a str>,
    pub state: Option<&'a str>,
}

/// Write one structured event to the local log.
///
/// Development builds write to stderr. Release builds append to the runtime
/// log file once [`set_log_file`] has been called, and hold earlier lines until
/// then. Logging stays on the machine. There is no remote sink and no telemetry.
pub fn emit(fields: &LogFields<'_>) {
    let line = format_event(SystemTime::now(), fields);
    #[cfg(debug_assertions)]
    {
        let _ = writeln!(io::stderr(), "{line}");
    }
    #[cfg(not(debug_assertions))]
    {
        write_log_line(&line);
    }
}

/// Remember the release log file and flush lines emitted before it was known.
///
/// Development builds keep using stderr, so this is a no-op there.
pub fn set_log_file(path: impl Into<std::path::PathBuf>) {
    #[cfg(debug_assertions)]
    {
        let _ = path.into();
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = LOG_PATH.set(path.into());
        flush_pending();
    }
}

#[cfg(not(debug_assertions))]
static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
#[cfg(not(debug_assertions))]
static PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[cfg(not(debug_assertions))]
fn write_log_line(line: &str) {
    let Some(path) = LOG_PATH.get() else {
        if let Ok(mut pending) = PENDING.lock() {
            if pending.len() < 256 {
                pending.push(line.to_string());
            }
        }
        return;
    };
    append_line(path, line);
}

#[cfg(not(debug_assertions))]
fn flush_pending() {
    let Some(path) = LOG_PATH.get() else {
        return;
    };
    let pending = PENDING
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .drain(..)
        .collect::<Vec<_>>();
    for line in pending {
        append_line(path, &line);
    }
}

#[cfg(not(debug_assertions))]
fn append_line(path: &std::path::Path, line: &str) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}

pub(crate) fn format_event(timestamp: SystemTime, fields: &LogFields<'_>) -> String {
    format!(
        "timestamp={} level={} component={} service_id={} version={} platform={} state={} message={} error={}",
        format_rfc3339_utc(timestamp),
        fields.level.as_str(),
        fields.component,
        display_field(fields.service_id),
        display_field(fields.version),
        display_field(fields.platform),
        display_field(fields.state),
        display_field(Some(fields.message)),
        display_field(fields.error),
    )
}

fn display_field(value: Option<&str>) -> String {
    match value {
        None => "-".to_string(),
        Some(value) => {
            if value
                .chars()
                .any(|character| character.is_whitespace() || character == '"' || character == '=')
            {
                let mut quoted = String::with_capacity(value.len() + 2);
                quoted.push('"');
                for character in value.chars() {
                    if character == '\\' || character == '"' {
                        quoted.push('\\');
                    }
                    quoted.push(character);
                }
                quoted.push('"');
                quoted
            } else if value.is_empty() {
                "\"\"".to_string()
            } else {
                value.to_string()
            }
        }
    }
}

fn format_rfc3339_utc(time: SystemTime) -> String {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();
    let days = i64::try_from(secs / 86_400).unwrap_or(i64::MAX);
    let time_of_day = u32::try_from(secs % 86_400).unwrap_or(0);
    let (year, month, day) = civil_from_days(days);
    let hour = time_of_day / 3_600;
    let minute = (time_of_day % 3_600) / 60;
    let second = time_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Howard Hinnant's `civil_from_days`, with `days` counted from 1970-01-01.
fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = u64::try_from(z - era * 146_097).unwrap_or(0);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = i64::try_from(yoe).unwrap_or(0) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        year += 1;
    }
    (
        i32::try_from(year).unwrap_or(0),
        u32::try_from(month).unwrap_or(0),
        u32::try_from(day).unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::{format_event, format_rfc3339_utc, Level, LogFields};
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn formats_known_utc_timestamps() {
        assert_eq!(format_rfc3339_utc(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            format_rfc3339_utc(UNIX_EPOCH + Duration::from_secs(86_400)),
            "1970-01-02T00:00:00.000Z"
        );
        assert_eq!(
            format_rfc3339_utc(UNIX_EPOCH + Duration::from_secs(3_661)),
            "1970-01-01T01:01:01.000Z"
        );
        assert_eq!(
            format_rfc3339_utc(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            "2023-11-14T22:13:20.000Z"
        );
        assert_eq!(
            format_rfc3339_utc(UNIX_EPOCH + Duration::from_secs(1_791_019_800)),
            "2026-10-03T09:30:00.000Z"
        );
    }

    #[test]
    fn event_line_contains_required_fields() {
        let line = format_event(
            UNIX_EPOCH,
            &LogFields {
                level: Level::Info,
                component: "supervisor",
                service_id: Some("screenshot"),
                version: Some("0.1.0"),
                platform: Some("windows-x86_64"),
                message: "startup complete",
                error: None,
                state: Some("Running"),
            },
        );
        assert!(line.contains("timestamp=1970-01-01T00:00:00.000Z"));
        assert!(line.contains("level=INFO"));
        assert!(line.contains("component=supervisor"));
        assert!(line.contains("service_id=screenshot"));
        assert!(line.contains("version=0.1.0"));
        assert!(line.contains("platform=windows-x86_64"));
        assert!(line.contains("state=Running"));
        assert!(line.contains("message=\"startup complete\""));
        assert!(line.contains("error=-"));
    }
}
