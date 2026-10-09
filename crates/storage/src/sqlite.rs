//! One SQLite database for structured tracking records.
//!
//! The connection stays open. Writes use transactions and parameterized
//! statements. `synchronous=OFF` is not used. A failed open does not delete
//! the file.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::{
    log_storage, ActivityCounts, ApplicationSegment, ArtifactKind, ArtifactRequest, IntervalDraft,
    IntervalParts, Level, LocalStore, PendingRecord, StorageError, SyncStatus, UrlSanitizer,
    UrlSegment,
};

pub const DEFAULT_MAX_LOCAL_STORAGE_BYTES: u64 = 1024 * 1024 * 1024;
const SCHEMA_VERSION: &str = "1";
const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

static IDS: AtomicU64 = AtomicU64::new(1);

pub struct StoreOptions {
    pub max_local_storage_bytes: u64,
    pub config_version: String,
    pub user_id: Option<String>,
    pub sanitize_url: UrlSanitizer,
}

pub struct SqliteStore {
    connection: Mutex<Connection>,
    data_dir: PathBuf,
    database_path: PathBuf,
    max_bytes: u64,
    sanitize_url: UrlSanitizer,
    warned: AtomicBool,
}

impl SqliteStore {
    pub fn open(data_dir: &Path, options: StoreOptions) -> Result<Self, StorageError> {
        if !data_dir.is_absolute() {
            return Err(StorageError::unavailable(
                "storage directory must be an absolute data path",
            ));
        }
        if options.max_local_storage_bytes == 0 {
            return Err(StorageError::rejected(
                "storage limit must be greater than zero",
            ));
        }
        fs::create_dir_all(data_dir)
            .map_err(|_| StorageError::unavailable("storage directory could not be created"))?;
        let data_dir = data_dir
            .canonicalize()
            .map_err(|_| StorageError::unavailable("storage directory could not be created"))?;
        let database_path = data_dir.join("wind.db");
        let connection = Connection::open(&database_path).map_err(|_| {
            log_storage(
                Level::Error,
                "storage_write_failed",
                Some("database could not be opened"),
            );
            StorageError::unavailable("database could not be opened")
        })?;
        configure(&connection)?;
        initialize(&connection, &options)?;
        let store = Self {
            connection: Mutex::new(connection),
            data_dir: data_dir.to_path_buf(),
            database_path,
            max_bytes: options.max_local_storage_bytes,
            sanitize_url: options.sanitize_url,
            warned: AtomicBool::new(false),
        };
        store.note_orphans()?;
        log_storage(Level::Info, "storage_database_opened", None);
        log_storage(Level::Info, "storage_initialized", None);
        Ok(store)
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    fn note_orphans(&self) -> Result<(), StorageError> {
        let conn = self.lock();
        for directory in ["screenshots", "videos"] {
            let dir = self.data_dir.join(directory);
            let entries = match fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let path = path.canonicalize().unwrap_or(path);
                let text = path.to_string_lossy().to_string();
                let known: Option<i64> = conn
                    .query_row(
                        "SELECT 1 FROM artifacts WHERE local_path = ?1
                         UNION SELECT 1 FROM orphan_artifacts WHERE local_path = ?1",
                        params![text],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|_| StorageError::unavailable("orphan lookup failed"))?;
                if known.is_some() {
                    continue;
                }
                let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                conn.execute(
                    "INSERT INTO orphan_artifacts (local_path, byte_size, detected_at) VALUES (?1, ?2, ?3)",
                    params![text, i64_from_u64(size)?, unix_ms(SystemTime::now())?],
                )
                .map_err(|_| StorageError::unavailable("orphan record could not be saved"))?;
            }
        }
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn ensure_capacity(&self, conn: &Connection, extra: u64) -> Result<(), StorageError> {
        let used = usage_bytes(conn, &self.database_path)?;
        if used.saturating_mul(10) >= self.max_bytes.saturating_mul(9)
            && self
                .warned
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            log_storage(Level::Warn, "storage_disk_limit_reached", None);
        }
        if used.saturating_add(extra) > self.max_bytes {
            log_storage(Level::Error, "storage_disk_limit_reached", None);
            return Err(StorageError::exhausted(
                "local storage limit would be exceeded",
            ));
        }
        Ok(())
    }
}

impl LocalStore for SqliteStore {
    fn create_interval(&self, started_at: SystemTime) -> Result<String, StorageError> {
        let mut conn = self.lock();
        self.ensure_capacity(&conn, 0)?;
        let id = new_id();
        let started = unix_ms(started_at)?;
        let tx = conn.transaction().map_err(|_| write_failed())?;
        tx.execute(
            "INSERT INTO intervals (
                id, business_date, started_at, ended_at, duration_seconds, created_at,
                sync_status, previous_hash, record_hash
             ) VALUES (?1, ?2, ?3, ?3, 0, ?3, 'open', '', '')",
            params![id, utc_date(started)?, started],
        )
        .map_err(|_| write_failed())?;
        tx.commit().map_err(|_| write_failed())?;
        Ok(id)
    }

    fn finish_interval(
        &self,
        id: &str,
        ended_at: SystemTime,
        parts: IntervalParts,
    ) -> Result<(), StorageError> {
        let mut conn = self.lock();
        self.ensure_capacity(&conn, 0)?;
        let ended = unix_ms(ended_at)?;
        let tx = conn.transaction().map_err(|_| write_failed())?;
        let (started, status): (i64, String) = tx
            .query_row(
                "SELECT started_at, sync_status FROM intervals WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| StorageError::rejected("interval does not exist"))?;
        if status != "open" {
            return Err(StorageError::rejected("interval is already finished"));
        }
        if ended < started {
            return Err(StorageError::rejected("interval end is before its start"));
        }
        let duration = (ended - started) / 1000;
        let business_date = utc_date(started)?;
        tx.execute(
            "UPDATE intervals
             SET ended_at = ?1, duration_seconds = ?2, business_date = ?3, sync_status = 'pending'
             WHERE id = ?4 AND sync_status = 'open'",
            params![ended, duration, business_date, id],
        )
        .map_err(|_| write_failed())?;
        let interval_canonical =
            format!("interval|{id}|{business_date}|{started}|{ended}|{duration}");
        append_chain(&tx, "interval", id, &interval_canonical, ended)?;
        let (previous_hash, record_hash) = chain_tip(&tx, "interval", id)?;
        tx.execute(
            "UPDATE intervals SET previous_hash = ?1, record_hash = ?2 WHERE id = ?3",
            params![previous_hash, record_hash, id],
        )
        .map_err(|_| write_failed())?;
        if let Some(activity) = parts.activity {
            insert_activity(&tx, id, &activity)?;
        }
        if let Some(application) = parts.application {
            insert_application(&tx, id, &application)?;
        }
        if let Some(url) = parts.url {
            insert_url(&tx, id, &url, self.sanitize_url)?;
        }
        tx.commit().map_err(|_| write_failed())?;
        Ok(())
    }

    fn record_interval(&self, draft: IntervalDraft) -> Result<String, StorageError> {
        let mut conn = self.lock();
        self.ensure_capacity(&conn, 0)?;
        let id = new_id();
        let started = unix_ms(draft.started_at)?;
        let ended = unix_ms(draft.ended_at)?;
        if ended < started {
            return Err(StorageError::rejected("interval end is before its start"));
        }
        let duration = (ended - started) / 1000;
        let business_date = utc_date(started)?;
        let tx = conn.transaction().map_err(|_| write_failed())?;
        tx.execute(
            "INSERT INTO intervals (
                id, business_date, started_at, ended_at, duration_seconds, created_at,
                sync_status, previous_hash, record_hash
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?3, 'pending', '', '')",
            params![id, business_date, started, ended, duration],
        )
        .map_err(|_| write_failed())?;
        let interval_canonical =
            format!("interval|{id}|{business_date}|{started}|{ended}|{duration}");
        append_chain(&tx, "interval", &id, &interval_canonical, started)?;
        let (previous_hash, record_hash) = chain_tip(&tx, "interval", &id)?;
        tx.execute(
            "UPDATE intervals SET previous_hash = ?1, record_hash = ?2 WHERE id = ?3",
            params![previous_hash, record_hash, id],
        )
        .map_err(|_| write_failed())?;
        if let Some(activity) = &draft.activity {
            insert_activity(&tx, &id, activity)?;
        }
        if let Some(application) = &draft.application {
            insert_application(&tx, &id, application)?;
        }
        if let Some(url) = &draft.url {
            insert_url(&tx, &id, url, self.sanitize_url)?;
        }
        tx.commit().map_err(|_| write_failed())?;
        Ok(id)
    }

    fn register_artifact(&self, request: ArtifactRequest) -> Result<String, StorageError> {
        let path = request
            .path
            .canonicalize()
            .map_err(|_| StorageError::rejected("artifact path is not available"))?;
        if !path.starts_with(&self.data_dir) {
            return Err(StorageError::rejected(
                "artifact is outside the data directory",
            ));
        }
        let expected = match request.kind {
            ArtifactKind::Screenshot => "screenshots",
            ArtifactKind::Video => "videos",
        };
        if !path
            .parent()
            .and_then(|parent| parent.file_name())
            .is_some_and(|name| name == expected)
        {
            return Err(StorageError::rejected("artifact directory is not accepted"));
        }
        let (size, digest) = hash_file(&path)?;
        let captured = unix_ms(request.captured_at)?;
        let mut conn = self.lock();
        self.ensure_capacity(&conn, size)?;
        let interval_id = new_id();
        let artifact_id = new_id();
        let tx = conn.transaction().map_err(|_| write_failed())?;
        tx.execute(
            "INSERT INTO intervals (
                id, business_date, started_at, ended_at, duration_seconds, created_at,
                sync_status, previous_hash, record_hash
             ) VALUES (?1, ?2, ?3, ?3, 0, ?3, 'pending', '', '')",
            params![interval_id, utc_date(captured)?, captured],
        )
        .map_err(|_| write_failed())?;
        let business_date = utc_date(captured)?;
        let interval_canonical =
            format!("interval|{interval_id}|{business_date}|{captured}|{captured}|0");
        append_chain(&tx, "interval", &interval_id, &interval_canonical, captured)?;
        let (previous_hash, record_hash) = chain_tip(&tx, "interval", &interval_id)?;
        tx.execute(
            "UPDATE intervals SET previous_hash = ?1, record_hash = ?2 WHERE id = ?3",
            params![previous_hash, record_hash, interval_id],
        )
        .map_err(|_| write_failed())?;
        let path_text = path.to_string_lossy().to_string();
        tx.execute(
            "INSERT INTO artifacts (
                id, interval_id, artifact_type, local_path, file_size, sha256, captured_at,
                remote_url, upload_status, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, 'pending', ?7)",
            params![
                artifact_id,
                interval_id,
                request.kind.as_str(),
                path_text,
                i64_from_u64(size)?,
                digest,
                captured
            ],
        )
        .map_err(|_| write_failed())?;
        let artifact_canonical = format!(
            "artifact|{artifact_id}|{interval_id}|{}|{digest}|{size}|{captured}",
            request.kind.as_str()
        );
        append_chain(&tx, "artifact", &artifact_id, &artifact_canonical, captured)?;
        tx.execute(
            "DELETE FROM orphan_artifacts WHERE local_path = ?1",
            params![path_text],
        )
        .map_err(|_| write_failed())?;
        tx.commit().map_err(|_| write_failed())?;
        log_storage(Level::Info, "storage_artifact_registered", None);
        Ok(artifact_id)
    }

    fn attach_artifact(
        &self,
        interval_id: &str,
        request: ArtifactRequest,
    ) -> Result<String, StorageError> {
        let path = request
            .path
            .canonicalize()
            .map_err(|_| StorageError::rejected("artifact path is not available"))?;
        if !path.starts_with(&self.data_dir) {
            return Err(StorageError::rejected(
                "artifact is outside the data directory",
            ));
        }
        let expected = match request.kind {
            ArtifactKind::Screenshot => "screenshots",
            ArtifactKind::Video => "videos",
        };
        if !path
            .parent()
            .and_then(|parent| parent.file_name())
            .is_some_and(|name| name == expected)
        {
            return Err(StorageError::rejected("artifact directory is not accepted"));
        }
        let (size, digest) = hash_file(&path)?;
        let captured = unix_ms(request.captured_at)?;
        let mut conn = self.lock();
        self.ensure_capacity(&conn, size)?;
        let tx = conn.transaction().map_err(|_| write_failed())?;
        let found: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM intervals WHERE id = ?1",
                params![interval_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| write_failed())?;
        if found.is_none() {
            return Err(StorageError::rejected("interval does not exist"));
        }
        let artifact_id = new_id();
        let path_text = path.to_string_lossy().to_string();
        tx.execute(
            "INSERT INTO artifacts (
                id, interval_id, artifact_type, local_path, file_size, sha256, captured_at,
                remote_url, upload_status, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, 'pending', ?7)",
            params![
                artifact_id,
                interval_id,
                request.kind.as_str(),
                path_text,
                i64_from_u64(size)?,
                digest,
                captured
            ],
        )
        .map_err(|_| write_failed())?;
        let artifact_canonical = format!(
            "artifact|{artifact_id}|{interval_id}|{}|{digest}|{size}|{captured}",
            request.kind.as_str()
        );
        append_chain(&tx, "artifact", &artifact_id, &artifact_canonical, captured)?;
        tx.execute(
            "DELETE FROM orphan_artifacts WHERE local_path = ?1",
            params![path_text],
        )
        .map_err(|_| write_failed())?;
        tx.commit().map_err(|_| write_failed())?;
        log_storage(Level::Info, "storage_artifact_registered", None);
        Ok(artifact_id)
    }

    fn pending_records(&self) -> Result<Vec<PendingRecord>, StorageError> {
        let conn = self.lock();
        let mut statement = conn
            .prepare(
                "SELECT sequence_number, record_type, record_id, previous_hash, record_hash, sync_status
                 FROM tracking_sequence WHERE sync_status = 'pending' ORDER BY sequence_number",
            )
            .map_err(|_| StorageError::unavailable("pending records could not be read"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(|_| StorageError::unavailable("pending records could not be read"))?;
        let mut pending = Vec::new();
        for row in rows {
            let (sequence, record_type, record_id, previous_hash, record_hash, status) =
                row.map_err(|_| StorageError::unavailable("pending records could not be read"))?;
            let Some(sync_status) = SyncStatus::parse(&status) else {
                return Err(StorageError::integrity("sync status is not recognized"));
            };
            pending.push(PendingRecord {
                sequence_number: u64::try_from(sequence).unwrap_or(0),
                record_type,
                record_id,
                previous_hash,
                record_hash,
                sync_status,
            });
        }
        Ok(pending)
    }

    fn acknowledge(&self, sequence_number: u64) -> Result<(), StorageError> {
        let sequence = i64_from_u64(sequence_number)?;
        let mut conn = self.lock();
        let tx = conn.transaction().map_err(|_| write_failed())?;
        let found: Option<(String, String)> = tx
            .query_row(
                "SELECT record_type, record_id FROM tracking_sequence
                 WHERE sequence_number = ?1 AND sync_status = 'pending'",
                params![sequence],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|_| write_failed())?;
        let Some((record_type, record_id)) = found else {
            return Err(StorageError::rejected("pending record was not found"));
        };
        tx.execute(
            "UPDATE tracking_sequence SET sync_status = 'uploaded' WHERE sequence_number = ?1",
            params![sequence],
        )
        .map_err(|_| write_failed())?;
        if record_type == "interval" {
            tx.execute(
                "UPDATE intervals SET sync_status = 'uploaded' WHERE id = ?1",
                params![record_id],
            )
            .map_err(|_| write_failed())?;
        } else if record_type == "artifact" {
            tx.execute(
                "UPDATE artifacts SET upload_status = 'uploaded' WHERE id = ?1",
                params![record_id],
            )
            .map_err(|_| write_failed())?;
        }
        tx.commit().map_err(|_| write_failed())?;
        Ok(())
    }

    fn validate_chain(&self) -> Result<(), StorageError> {
        let conn = self.lock();
        validate_chain(&conn)
    }
}

#[cfg(test)]
struct ArtifactRow {
    interval_id: String,
    kind: String,
    remote_url: Option<String>,
    upload_status: String,
    file_size: i64,
    sha256: String,
}

impl SqliteStore {
    #[cfg(test)]
    fn tamper_keyboard(&self, interval_id: &str, value: i64) -> Result<(), StorageError> {
        let conn = self.lock();
        conn.execute(
            "UPDATE activity SET keyboard_events = ?1 WHERE interval_id = ?2",
            params![value, interval_id],
        )
        .map_err(|_| write_failed())?;
        Ok(())
    }

    #[cfg(test)]
    fn set_schema_version(&self, version: &str) -> Result<(), StorageError> {
        let conn = self.lock();
        conn.execute(
            "UPDATE metadata SET value = ?1 WHERE key = 'schema_version'",
            params![version],
        )
        .map_err(|_| write_failed())?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn testing_activity(
        &self,
        interval_id: &str,
    ) -> Result<Option<ActivityCounts>, StorageError> {
        let conn = self.lock();
        let row = conn
            .query_row(
                "SELECT keyboard_events, mouse_moves, mouse_clicks, mouse_scrolls
                 FROM activity WHERE interval_id = ?1",
                params![interval_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| StorageError::unavailable("activity could not be read"))?;
        Ok(
            row.map(|(keyboard, moves, clicks, scrolls)| ActivityCounts {
                keyboard_events: keyboard as u64,
                mouse_moves: moves as u64,
                mouse_clicks: clicks as u64,
                mouse_scrolls: scrolls as u64,
            }),
        )
    }

    #[cfg(test)]
    fn testing_url(&self, interval_id: &str) -> Result<Option<String>, StorageError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT url FROM url_usage WHERE interval_id = ?1",
            params![interval_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| StorageError::unavailable("url could not be read"))
    }

    #[cfg(test)]
    fn testing_artifact(&self, id: &str) -> Result<Option<ArtifactRow>, StorageError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT interval_id, artifact_type, remote_url, upload_status, file_size, sha256
             FROM artifacts WHERE id = ?1",
            params![id],
            |row| {
                Ok(ArtifactRow {
                    interval_id: row.get(0)?,
                    kind: row.get(1)?,
                    remote_url: row.get(2)?,
                    upload_status: row.get(3)?,
                    file_size: row.get(4)?,
                    sha256: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(|_| StorageError::unavailable("artifact could not be read"))
    }

    #[cfg(test)]
    fn pragma(&self, name: &str) -> Result<String, StorageError> {
        let conn = self.lock();
        let sql = format!("PRAGMA {name}");
        conn.query_row(&sql, [], |row| row.get::<_, String>(0))
            .or_else(|_| {
                conn.query_row(&sql, [], |row| {
                    row.get::<_, i64>(0).map(|value| value.to_string())
                })
            })
            .map_err(|_| StorageError::unavailable("pragma could not be read"))
    }
}

fn configure(connection: &Connection) -> Result<(), StorageError> {
    connection
        .busy_timeout(std::time::Duration::from_millis(5_000))
        .map_err(|_| StorageError::unavailable("database busy timeout could not be set"))?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON")
        .map_err(|_| StorageError::unavailable("foreign keys could not be enabled"))?;
    let foreign_keys: i64 = connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .map_err(|_| StorageError::unavailable("foreign keys could not be enabled"))?;
    if foreign_keys != 1 {
        return Err(StorageError::unavailable("foreign keys are not enabled"));
    }
    let journal: String = connection
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .map_err(|_| StorageError::unavailable("journal mode could not be set"))?;
    if !journal.eq_ignore_ascii_case("wal") {
        return Err(StorageError::unavailable(
            "database is not using write-ahead logging",
        ));
    }
    connection
        .execute_batch("PRAGMA synchronous = FULL")
        .map_err(|_| StorageError::unavailable("synchronous mode could not be set"))?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|_| {
            log_storage(
                Level::Error,
                "storage_integrity_check_failed",
                Some("integrity check could not run"),
            );
            StorageError::integrity("integrity check could not run")
        })?;
    if integrity != "ok" {
        log_storage(
            Level::Error,
            "storage_integrity_check_failed",
            Some("database integrity check failed"),
        );
        return Err(StorageError::integrity("database integrity check failed"));
    }
    Ok(())
}

fn initialize(connection: &Connection, options: &StoreOptions) -> Result<(), StorageError> {
    connection
        .execute_batch(SCHEMA)
        .map_err(|_| StorageError::unavailable("database schema could not be created"))?;
    let existing: Option<String> = connection
        .query_row(
            "SELECT value FROM metadata WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| StorageError::unavailable("schema version could not be read"))?;
    match existing.as_deref() {
        None => {
            connection
                .execute(
                    "INSERT INTO metadata (key, value) VALUES ('schema_version', ?1)",
                    params![SCHEMA_VERSION],
                )
                .map_err(|_| StorageError::unavailable("schema version could not be saved"))?;
            connection
                .execute(
                    "INSERT INTO metadata (key, value) VALUES ('max_local_storage_bytes', ?1)",
                    params![options.max_local_storage_bytes.to_string()],
                )
                .map_err(|_| StorageError::unavailable("storage limit could not be saved"))?;
        }
        Some(SCHEMA_VERSION) => {}
        Some(_) => {
            return Err(StorageError::unavailable(
                "database schema version is not supported",
            ));
        }
    }
    let now = unix_ms(SystemTime::now())?;
    connection
        .execute(
            "INSERT INTO agent_config (
                id, config_version, user_id, organization_id, device_id, timezone,
                interval_duration_seconds, enabled_features, updated_at
             ) VALUES (1, ?1, ?2, NULL, NULL, NULL, NULL, NULL, ?3)
             ON CONFLICT(id) DO UPDATE SET
                config_version = excluded.config_version,
                user_id = excluded.user_id,
                updated_at = excluded.updated_at",
            params![options.config_version, options.user_id, now],
        )
        .map_err(|_| StorageError::unavailable("configuration could not be saved"))?;
    Ok(())
}

fn insert_activity(
    tx: &rusqlite::Transaction<'_>,
    interval_id: &str,
    activity: &ActivityCounts,
) -> Result<(), StorageError> {
    let keyboard = i64_from_u64(activity.keyboard_events)?;
    let moves = i64_from_u64(activity.mouse_moves)?;
    let clicks = i64_from_u64(activity.mouse_clicks)?;
    let scrolls = i64_from_u64(activity.mouse_scrolls)?;
    tx.execute(
        "INSERT INTO activity (
            interval_id, keyboard_events, mouse_moves, mouse_clicks, mouse_scrolls
         ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![interval_id, keyboard, moves, clicks, scrolls],
    )
    .map_err(|_| write_failed())?;
    let canonical = format!("activity|{interval_id}|{keyboard}|{moves}|{clicks}|{scrolls}");
    append_chain(
        tx,
        "activity",
        interval_id,
        &canonical,
        unix_ms(SystemTime::now())?,
    )?;
    Ok(())
}

fn insert_application(
    tx: &rusqlite::Transaction<'_>,
    interval_id: &str,
    application: &ApplicationSegment,
) -> Result<(), StorageError> {
    if application.application_id.is_empty() || application.application_id.contains(['\n', '\r']) {
        return Err(StorageError::rejected("application identity is empty"));
    }
    let started = unix_ms(application.started_at)?;
    let ended = unix_ms(application.ended_at)?;
    if ended < started {
        return Err(StorageError::rejected(
            "application segment end is before its start",
        ));
    }
    let duration = (ended - started) / 1000;
    let id = new_id();
    tx.execute(
        "INSERT INTO application_usage (
            id, interval_id, application_id, started_at, ended_at, duration_seconds
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            id,
            interval_id,
            application.application_id,
            started,
            ended,
            duration
        ],
    )
    .map_err(|_| write_failed())?;
    let canonical = format!(
        "application|{id}|{interval_id}|{}|{started}|{ended}|{duration}",
        application.application_id
    );
    append_chain(tx, "application", &id, &canonical, ended)?;
    Ok(())
}

fn insert_url(
    tx: &rusqlite::Transaction<'_>,
    interval_id: &str,
    segment: &UrlSegment,
    sanitize: UrlSanitizer,
) -> Result<(), StorageError> {
    let url = sanitize(&segment.url).map_err(|_| StorageError::rejected("url was rejected"))?;
    let started = unix_ms(segment.started_at)?;
    let ended = unix_ms(segment.ended_at)?;
    if ended < started {
        return Err(StorageError::rejected(
            "url segment end is before its start",
        ));
    }
    let duration = (ended - started) / 1000;
    let id = new_id();
    tx.execute(
        "INSERT INTO url_usage (
            id, interval_id, url, started_at, ended_at, duration_seconds
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, interval_id, url, started, ended, duration],
    )
    .map_err(|_| write_failed())?;
    let canonical = format!("url|{id}|{interval_id}|{url}|{started}|{ended}|{duration}");
    append_chain(tx, "url", &id, &canonical, ended)?;
    Ok(())
}

fn append_chain(
    tx: &rusqlite::Transaction<'_>,
    record_type: &str,
    record_id: &str,
    canonical: &str,
    created_at: i64,
) -> Result<(), StorageError> {
    let previous = tx
        .query_row(
            "SELECT record_hash FROM tracking_sequence ORDER BY sequence_number DESC LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| write_failed())?
        .unwrap_or_else(|| GENESIS.to_string());
    let record_hash = chain_hash(&previous, canonical);
    let next = tx
        .query_row(
            "SELECT COALESCE(MAX(sequence_number), 0) + 1 FROM tracking_sequence",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| write_failed())?;
    tx.execute(
        "INSERT INTO tracking_sequence (
            sequence_number, record_type, record_id, previous_hash, record_hash,
            sync_status, attempt_count, last_attempt_at, next_attempt_at, last_error, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', 0, NULL, NULL, NULL, ?6)",
        params![
            next,
            record_type,
            record_id,
            previous,
            record_hash,
            created_at
        ],
    )
    .map_err(|_| write_failed())?;
    Ok(())
}

fn chain_tip(
    tx: &rusqlite::Transaction<'_>,
    record_type: &str,
    record_id: &str,
) -> Result<(String, String), StorageError> {
    tx.query_row(
        "SELECT previous_hash, record_hash FROM tracking_sequence
         WHERE record_type = ?1 AND record_id = ?2
         ORDER BY sequence_number DESC LIMIT 1",
        params![record_type, record_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .map_err(|_| write_failed())
}

fn validate_chain(connection: &Connection) -> Result<(), StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT sequence_number, record_type, record_id, previous_hash, record_hash
             FROM tracking_sequence ORDER BY sequence_number",
        )
        .map_err(|_| StorageError::integrity("hash chain could not be read"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(|_| StorageError::integrity("hash chain could not be read"))?;
    let mut expected_previous = GENESIS.to_string();
    for (expected_sequence, row) in (1i64..).zip(rows) {
        let (sequence, record_type, record_id, previous_hash, record_hash) =
            row.map_err(|_| StorageError::integrity("hash chain could not be read"))?;
        if sequence != expected_sequence || previous_hash != expected_previous {
            log_storage(
                Level::Error,
                "storage_integrity_check_failed",
                Some("hash chain link does not match"),
            );
            return Err(StorageError::integrity("hash chain link does not match"));
        }
        let canonical = canonical_for(connection, &record_type, &record_id)?;
        let computed = chain_hash(&previous_hash, &canonical);
        if computed != record_hash {
            log_storage(
                Level::Error,
                "storage_integrity_check_failed",
                Some("hash chain record does not match"),
            );
            return Err(StorageError::integrity("hash chain record does not match"));
        }
        expected_previous = record_hash;
    }
    Ok(())
}

fn canonical_for(
    connection: &Connection,
    record_type: &str,
    record_id: &str,
) -> Result<String, StorageError> {
    let missing = || StorageError::integrity("hash chain record is missing its row");
    match record_type {
        "interval" => connection
            .query_row(
                "SELECT business_date, started_at, ended_at, duration_seconds FROM intervals WHERE id = ?1",
                params![record_id],
                |row| {
                    Ok(format!(
                        "interval|{record_id}|{}|{}|{}|{}",
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?
                    ))
                },
            )
            .map_err(|_| missing()),
        "activity" => connection
            .query_row(
                "SELECT keyboard_events, mouse_moves, mouse_clicks, mouse_scrolls FROM activity WHERE interval_id = ?1",
                params![record_id],
                |row| {
                    Ok(format!(
                        "activity|{record_id}|{}|{}|{}|{}",
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?
                    ))
                },
            )
            .map_err(|_| missing()),
        "application" => connection
            .query_row(
                "SELECT interval_id, application_id, started_at, ended_at, duration_seconds
                 FROM application_usage WHERE id = ?1",
                params![record_id],
                |row| {
                    Ok(format!(
                        "application|{record_id}|{}|{}|{}|{}|{}",
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?
                    ))
                },
            )
            .map_err(|_| missing()),
        "url" => connection
            .query_row(
                "SELECT interval_id, url, started_at, ended_at, duration_seconds FROM url_usage WHERE id = ?1",
                params![record_id],
                |row| {
                    Ok(format!(
                        "url|{record_id}|{}|{}|{}|{}|{}",
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?
                    ))
                },
            )
            .map_err(|_| missing()),
        "artifact" => connection
            .query_row(
                "SELECT interval_id, artifact_type, sha256, file_size, captured_at FROM artifacts WHERE id = ?1",
                params![record_id],
                |row| {
                    Ok(format!(
                        "artifact|{record_id}|{}|{}|{}|{}|{}",
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?
                    ))
                },
            )
            .map_err(|_| missing()),
        _ => Err(StorageError::integrity("hash chain record type is not recognized")),
    }
}

fn chain_hash(previous: &str, canonical: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(previous.as_bytes());
    hasher.update(b"\n");
    hasher.update(canonical.as_bytes());
    hex_encode(&hasher.finalize())
}

fn hash_file(path: &Path) -> Result<(u64, String), StorageError> {
    let mut file =
        File::open(path).map_err(|_| StorageError::unavailable("artifact could not be read"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    let mut size = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| StorageError::unavailable("artifact could not be read"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size = size.saturating_add(read as u64);
    }
    Ok((size, hex_encode(&hasher.finalize())))
}

fn usage_bytes(connection: &Connection, database_path: &Path) -> Result<u64, StorageError> {
    let pages: i64 = connection
        .query_row("PRAGMA page_count", [], |row| row.get(0))
        .map_err(|_| StorageError::unavailable("database size could not be read"))?;
    let page_size: i64 = connection
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .map_err(|_| StorageError::unavailable("database size could not be read"))?;
    let database = u64::try_from(pages.saturating_mul(page_size)).unwrap_or(0);
    let artifacts: i64 = connection
        .query_row(
            "SELECT COALESCE(SUM(file_size), 0) FROM artifacts",
            [],
            |row| row.get(0),
        )
        .map_err(|_| StorageError::unavailable("artifact size could not be read"))?;
    let orphans: i64 = connection
        .query_row(
            "SELECT COALESCE(SUM(byte_size), 0) FROM orphan_artifacts",
            [],
            |row| row.get(0),
        )
        .map_err(|_| StorageError::unavailable("orphan size could not be read"))?;
    let wal = fs::metadata(database_path.with_extension("db-wal"))
        .map(|meta| meta.len())
        .unwrap_or(0);
    Ok(database
        .saturating_add(u64::try_from(artifacts).unwrap_or(0))
        .saturating_add(u64::try_from(orphans).unwrap_or(0))
        .saturating_add(wal))
}

fn write_failed() -> StorageError {
    log_storage(
        Level::Error,
        "storage_write_failed",
        Some("database write failed"),
    );
    StorageError::unavailable("database write failed")
}

fn new_id() -> String {
    let number = IDS.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}{number:x}")
}

fn unix_ms(time: SystemTime) -> Result<i64, StorageError> {
    let millis = time
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StorageError::rejected("timestamp is before the unix epoch"))?
        .as_millis();
    i64::try_from(millis).map_err(|_| StorageError::rejected("timestamp is out of range"))
}

fn i64_from_u64(value: u64) -> Result<i64, StorageError> {
    i64::try_from(value).map_err(|_| StorageError::rejected("counter is out of range"))
}

fn utc_date(unix_ms: i64) -> Result<String, StorageError> {
    if unix_ms < 0 {
        return Err(StorageError::rejected("timestamp is before the unix epoch"));
    }
    let days = unix_ms / 86_400_000;
    let (year, month, day) = civil_from_days(days);
    Ok(format!("{year:04}-{month:02}-{day:02}"))
}

fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = (z - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = if month_part < 10 {
        month_part + 3
    } else {
        month_part - 9
    };
    let year = year_of_era as i64 + era * 400 + i64::from(month <= 2);
    (
        i32::try_from(year).unwrap_or(1970),
        month as u32,
        day as u32,
    )
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS intervals (
    id TEXT PRIMARY KEY,
    business_date TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    ended_at INTEGER NOT NULL,
    duration_seconds INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    sync_status TEXT NOT NULL,
    previous_hash TEXT NOT NULL,
    record_hash TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS activity (
    interval_id TEXT PRIMARY KEY,
    keyboard_events INTEGER NOT NULL,
    mouse_moves INTEGER NOT NULL,
    mouse_clicks INTEGER NOT NULL,
    mouse_scrolls INTEGER NOT NULL,
    FOREIGN KEY (interval_id) REFERENCES intervals(id)
);
CREATE TABLE IF NOT EXISTS application_usage (
    id TEXT PRIMARY KEY,
    interval_id TEXT NOT NULL,
    application_id TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    ended_at INTEGER NOT NULL,
    duration_seconds INTEGER NOT NULL,
    FOREIGN KEY (interval_id) REFERENCES intervals(id)
);
CREATE TABLE IF NOT EXISTS url_usage (
    id TEXT PRIMARY KEY,
    interval_id TEXT NOT NULL,
    url TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    ended_at INTEGER NOT NULL,
    duration_seconds INTEGER NOT NULL,
    FOREIGN KEY (interval_id) REFERENCES intervals(id)
);
CREATE TABLE IF NOT EXISTS artifacts (
    id TEXT PRIMARY KEY,
    interval_id TEXT NOT NULL,
    artifact_type TEXT NOT NULL,
    local_path TEXT NOT NULL,
    file_size INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    captured_at INTEGER NOT NULL,
    remote_url TEXT,
    upload_status TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (interval_id) REFERENCES intervals(id)
);
CREATE TABLE IF NOT EXISTS agent_config (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    config_version TEXT NOT NULL,
    user_id TEXT,
    organization_id TEXT,
    device_id TEXT,
    timezone TEXT,
    interval_duration_seconds INTEGER,
    enabled_features TEXT,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS tracking_sequence (
    sequence_number INTEGER PRIMARY KEY,
    record_type TEXT NOT NULL,
    record_id TEXT NOT NULL,
    previous_hash TEXT NOT NULL,
    record_hash TEXT NOT NULL,
    sync_status TEXT NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    last_attempt_at INTEGER,
    next_attempt_at INTEGER,
    last_error TEXT,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS orphan_artifacts (
    local_path TEXT PRIMARY KEY,
    byte_size INTEGER NOT NULL,
    detected_at INTEGER NOT NULL
);
";

#[cfg(test)]
mod tests {
    use super::{SqliteStore, StoreOptions, SCHEMA_VERSION};
    use crate::{
        ActivityCounts, ApplicationSegment, ArtifactKind, ArtifactRequest, IntervalDraft,
        IntervalParts, LocalStore, UrlSegment,
    };
    use std::fs;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn reject_urls(raw: &str) -> Result<String, String> {
        if raw == "https://example.com/path?a=1" {
            Ok(raw.to_string())
        } else {
            Err("url was rejected".to_string())
        }
    }

    fn options(limit: u64) -> StoreOptions {
        StoreOptions {
            max_local_storage_bytes: limit,
            config_version: "0.1.0".to_string(),
            user_id: None,
            sanitize_url: reject_urls,
        }
    }

    fn open(label: &str) -> (SqliteStore, std::path::PathBuf) {
        let directory =
            std::env::temp_dir().join(format!("wind-storage-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("dir");
        let store = SqliteStore::open(&directory, options(1024 * 1024)).expect("open");
        (store, directory)
    }

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn initialization_uses_wal_full_sync_and_schema_version() {
        let (store, directory) = open("init");
        assert_eq!(
            store.pragma("journal_mode").unwrap().to_ascii_lowercase(),
            "wal"
        );
        assert_eq!(store.pragma("synchronous").unwrap(), "2");
        assert_eq!(store.pragma("foreign_keys").unwrap(), "1");
        let version: String = store
            .lock()
            .query_row(
                "SELECT value FROM metadata WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        store.validate_chain().unwrap();
        drop(store);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn records_survive_reopen_and_stay_pending() {
        let (store, directory) = open("reopen");
        let id = store
            .record_interval(IntervalDraft {
                started_at: at(1_700_000_000),
                ended_at: at(1_700_000_300),
                activity: Some(ActivityCounts {
                    keyboard_events: 4,
                    mouse_moves: 5,
                    mouse_clicks: 2,
                    mouse_scrolls: 1,
                }),
                application: Some(ApplicationSegment {
                    application_id: "app.example".to_string(),
                    started_at: at(1_700_000_000),
                    ended_at: at(1_700_000_300),
                }),
                url: Some(UrlSegment {
                    url: "https://example.com/path?a=1".to_string(),
                    started_at: at(1_700_000_000),
                    ended_at: at(1_700_000_300),
                }),
            })
            .unwrap();
        assert_eq!(
            store.testing_activity(&id).unwrap().unwrap().mouse_clicks,
            2
        );
        let (business_date, duration): (String, i64) = store
            .lock()
            .query_row(
                "SELECT business_date, duration_seconds FROM intervals WHERE id = ?1",
                rusqlite::params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(business_date, "2023-11-14");
        assert_eq!(duration, 300);
        assert_eq!(
            store.testing_url(&id).unwrap().unwrap(),
            "https://example.com/path?a=1"
        );
        let path = store.database_path().to_path_buf();
        drop(store);
        let store = SqliteStore::open(&directory, options(1024 * 1024)).unwrap();
        assert_eq!(store.database_path(), path);
        let pending = store.pending_records().unwrap();
        assert!(pending
            .iter()
            .all(|record| record.sync_status == crate::SyncStatus::Pending));
        assert!(pending.len() >= 3);
        store.validate_chain().unwrap();
        drop(store);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn rejected_url_does_not_leave_an_interval() {
        let (store, directory) = open("reject");
        let error = store
            .record_interval(IntervalDraft {
                started_at: at(10),
                ended_at: at(20),
                activity: None,
                application: None,
                url: Some(UrlSegment {
                    url: "ftp://example.com".to_string(),
                    started_at: at(10),
                    ended_at: at(20),
                }),
            })
            .unwrap_err();
        assert!(error.to_string().contains("rejected"));
        let count: i64 = store
            .lock()
            .query_row("SELECT COUNT(*) FROM intervals", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
        drop(store);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn finishing_an_interval_twice_is_rejected() {
        let (store, directory) = open("finish");
        let id = store.create_interval(at(1_700_000_000)).unwrap();
        store
            .finish_interval(id.as_str(), at(1_700_000_010), IntervalParts::default())
            .unwrap();
        let error = store
            .finish_interval(id.as_str(), at(1_700_000_020), IntervalParts::default())
            .unwrap_err();
        assert!(error.to_string().contains("rejected"));
        drop(store);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn foreign_key_rejects_an_activity_without_an_interval() {
        let (store, directory) = open("fk");
        let error = store.lock().execute(
            "INSERT INTO activity (interval_id, keyboard_events, mouse_moves, mouse_clicks, mouse_scrolls)
             VALUES ('missing', 1, 0, 0, 0)",
            [],
        );
        assert!(error.is_err());
        drop(store);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn tampering_with_a_middle_record_breaks_the_chain() {
        let (store, directory) = open("chain");
        let mut ids = Vec::new();
        for index in 0..3 {
            let id = store
                .record_interval(IntervalDraft {
                    started_at: at(100 + index),
                    ended_at: at(110 + index),
                    activity: Some(ActivityCounts {
                        keyboard_events: index,
                        mouse_moves: 0,
                        mouse_clicks: 1,
                        mouse_scrolls: 0,
                    }),
                    application: None,
                    url: None,
                })
                .unwrap();
            ids.push(id);
        }
        store.validate_chain().unwrap();
        store.tamper_keyboard(&ids[1], 99).unwrap();
        assert!(store.validate_chain().is_err());
        drop(store);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn artifact_metadata_keeps_the_hash_and_leaves_the_remote_url_empty() {
        let (store, directory) = open("artifact");
        let screenshots = directory.join("screenshots");
        fs::create_dir_all(&screenshots).unwrap();
        let path = screenshots.join("one.png");
        fs::write(&path, b"png-bytes").unwrap();
        let id = store
            .register_artifact(ArtifactRequest {
                kind: ArtifactKind::Screenshot,
                path: path.clone(),
                captured_at: at(50),
            })
            .unwrap();
        let row = store.testing_artifact(&id).unwrap().unwrap();
        assert_eq!(row.kind, "screenshot");
        assert!(row.remote_url.is_none());
        assert_eq!(row.upload_status, "pending");
        assert_eq!(row.file_size, 9);
        let expected = super::hash_file(&path).unwrap().1;
        assert_eq!(row.sha256, expected);
        let bytes = fs::read(store.database_path()).unwrap();
        assert!(!bytes
            .windows(b"png-bytes".len())
            .any(|window| window == b"png-bytes"));
        drop(store);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn attach_artifact_links_the_existing_interval() {
        let (store, directory) = open("attach");
        let started = at(4_000);
        let ended = at(10_000);
        let interval_id = store
            .record_interval(IntervalDraft {
                started_at: started,
                ended_at: ended,
                activity: Some(ActivityCounts {
                    keyboard_events: 1,
                    mouse_moves: 2,
                    mouse_clicks: 3,
                    mouse_scrolls: 0,
                }),
                application: None,
                url: None,
            })
            .unwrap();
        let screenshots = directory.join("screenshots");
        fs::create_dir_all(&screenshots).unwrap();
        let path = screenshots.join("20231114-221324-000.png");
        fs::write(&path, b"png-bytes").unwrap();
        let artifact_id = store
            .attach_artifact(
                &interval_id,
                ArtifactRequest {
                    kind: ArtifactKind::Screenshot,
                    path: path.clone(),
                    captured_at: ended,
                },
            )
            .unwrap();
        let row = store.testing_artifact(&artifact_id).unwrap().unwrap();
        assert_eq!(row.interval_id, interval_id);
        assert_eq!(row.kind, "screenshot");
        assert_eq!(row.upload_status, "pending");
        assert!(row.remote_url.is_none());
        assert_eq!(row.file_size, 9);
        assert_eq!(row.sha256, super::hash_file(&path).unwrap().1);
        store.validate_chain().unwrap();
        assert!(store
            .attach_artifact(
                "missing",
                ArtifactRequest {
                    kind: ArtifactKind::Screenshot,
                    path,
                    captured_at: ended,
                },
            )
            .is_err());
        drop(store);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn unsupported_schema_is_left_in_place() {
        let (store, directory) = open("schema");
        let path = store.database_path().to_path_buf();
        store.set_schema_version("99").unwrap();
        drop(store);
        let error = match SqliteStore::open(&directory, options(1024)) {
            Ok(_) => panic!("unsupported schema was opened"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("not supported"));
        assert!(path.exists());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn concurrent_intervals_keep_one_valid_chain() {
        let directory =
            std::env::temp_dir().join(format!("wind-storage-concurrent-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let store =
            std::sync::Arc::new(SqliteStore::open(&directory, options(1024 * 1024)).unwrap());
        let mut handles = Vec::new();
        for index in 0..4 {
            let store = std::sync::Arc::clone(&store);
            handles.push(std::thread::spawn(move || {
                store
                    .record_interval(IntervalDraft {
                        started_at: at(1_000 + index),
                        ended_at: at(1_010 + index),
                        activity: Some(ActivityCounts {
                            keyboard_events: index,
                            mouse_moves: 0,
                            mouse_clicks: 0,
                            mouse_scrolls: 0,
                        }),
                        application: None,
                        url: None,
                    })
                    .unwrap();
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        store.validate_chain().unwrap();
        assert_eq!(store.pending_records().unwrap().len(), 8);
        drop(store);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn storage_limit_refuses_a_write_without_deleting_pending_rows() {
        let (mut store, directory) = open("quota");
        let id = store
            .record_interval(IntervalDraft {
                started_at: at(10),
                ended_at: at(20),
                activity: Some(ActivityCounts {
                    keyboard_events: 1,
                    mouse_moves: 0,
                    mouse_clicks: 0,
                    mouse_scrolls: 0,
                }),
                application: None,
                url: None,
            })
            .unwrap();
        store.max_bytes = 1;
        let error = store
            .record_interval(IntervalDraft {
                started_at: at(30),
                ended_at: at(40),
                activity: Some(ActivityCounts {
                    keyboard_events: 1,
                    mouse_moves: 0,
                    mouse_clicks: 0,
                    mouse_scrolls: 0,
                }),
                application: None,
                url: None,
            })
            .unwrap_err();
        assert!(error.to_string().contains("exhausted"));
        assert!(store.testing_activity(&id).unwrap().is_some());
        drop(store);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn activity_table_has_no_raw_input_columns() {
        let (store, directory) = open("columns");
        let names = {
            let connection = store.lock();
            let mut statement = connection.prepare("PRAGMA table_info(activity)").unwrap();
            let names: Vec<String> = statement
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .map(|name| name.unwrap())
                .collect();
            names
        };
        assert_eq!(
            names,
            vec![
                "interval_id",
                "keyboard_events",
                "mouse_moves",
                "mouse_clicks",
                "mouse_scrolls"
            ]
        );
        drop(store);
        let _ = fs::remove_dir_all(directory);
    }
}
