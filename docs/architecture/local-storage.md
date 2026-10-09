# Local storage

Wind keeps finalized tracking data on the machine. Structured rows go in one SQLite database. Screenshot and video bytes stay files. Secrets go to the operating-system credential store. This layer does not contact a network, upload artifacts, or authenticate.

```text
Service
   |
LocalStore
   |-- SQLite          <data_dir>/wind.db
   |-- filesystem      <data_dir>/screenshots, <data_dir>/videos
   \-- SecretStore     OS credential store
```

Services call storage operations. They do not receive a database connection and they do not run SQL.

The directory comes from the platform data path for `com.wind.agent`. Services do not hard-code a Windows path. Logs stay in the existing log directory.

## SQLite

The library is `rusqlite` 0.32 with its bundled SQLite. There is no ORM. Statements are parameterized.

Opening a database:

1. Open `wind.db`.
2. Set a 5 second busy timeout.
3. Turn foreign keys on.
4. Set the journal mode to WAL.
5. Set `synchronous=FULL`.
6. Create the schema if it is missing.
7. Require schema version `1`.
8. Run `PRAGMA integrity_check` and require `ok`.

`synchronous=OFF` is not used. Journaling stays on. A database that cannot be opened, has another schema version, or fails the integrity check is left in place. Wind reports the failure and does not delete or recreate the file.

One connection is kept open behind a mutex. A record does not open a new connection. Related rows for one interval commit in one transaction. A rejected URL rolls the interval back, so a failed write does not leave a partial interval.

## Schema version 1

| Table | Purpose |
| --- | --- |
| `metadata` | `schema_version` and `max_local_storage_bytes` |
| `intervals` | Bounded tracking period, UTC timestamps, sync status, chain hashes |
| `activity` | Four aggregate counters for one interval |
| `application_usage` | One finalized application segment |
| `url_usage` | One finalized sanitized URL segment |
| `artifacts` | Screenshot or video metadata. `remote_url` is NULL. `upload_status` starts at `pending` |
| `agent_config` | Non-secret configuration. One row |
| `tracking_sequence` | Hash-chain position and sync fields for each durable record |
| `orphan_artifacts` | Files found without metadata. They are recorded and not deleted |

`activity` has `keyboard_events`, `mouse_moves`, `mouse_clicks`, and `mouse_scrolls`. It has no key, character, coordinate, button, or wheel column. Phase 7 still counts each button-down as one click. Storage stores that counter and does not add a one-second click bucket.

Timestamps are UTC Unix milliseconds. `business_date` is the UTC calendar date of `started_at` (`YYYY-MM-DD`). Wind has no shift calendar, so this date is not a work-shift date.

`agent_config` stores the product version and the platform user id when the platform provides one. Windows does not provide a user id, so that column is NULL. Organization, device, timezone, interval duration, and enabled-feature columns exist and stay NULL. Wind does not invent a device id. Credentials are not stored in this table.

## What gets written

The supervisor closes a tracking interval on a session-relative 10-second UTC schedule and writes that window's activity delta, application segment, and URL segment. The first window starts when tracking starts and ends on the next 10-second UTC boundary. Later windows are 10 seconds. A shutdown in the middle of a window writes the partial window. An explicit activity snapshot still writes its own delta. While the interval engine is running, activity stop does not write that delta again. A zero delta on an explicit snapshot writes nothing.

Application and URL segments close when the identity or sanitized URL changes, and when the service stops. Identical observations are not written again. The URL is passed through the existing sanitizer before insert. A URL the sanitizer rejects is not stored.

A screenshot or video is hashed only after the file is complete. The hash and size are stored with the path. The file bytes are not inserted. If the process stops between writing the file and committing the row, the next open records the file as an orphan and leaves it on disk.

New finished records are `pending`. `acknowledge` can mark a sequence row `uploaded` for a later phase. Nothing in this phase uploads, retries, or watches the network. `attempt_count`, `last_attempt_at`, `next_attempt_at`, and `last_error` are present and unused.

## Tamper evidence

Each durable row is appended to `tracking_sequence`. The record hash is SHA-256 of the previous hash, a newline, and a fixed field order for that row. The first previous hash is 64 zero hex characters. Validation recomputes the canonical text from the source row. Changing a stored counter breaks the chain.

This detects ordinary modification and corruption. It does not make the database cryptographically immutable. An administrator who can replace both the database and the program can build a new chain. A future server checkpoint is the place to anchor the chain. This phase does not send the chain anywhere.

## Secrets

`SecretStore` is `get`, `set`, and `delete`. A missing name is an empty result. An unavailable store is an error. Wind does not fall back to a plaintext file.

| OS | Mechanism | Runtime status |
| --- | --- | --- |
| Windows | Credential Manager, generic credential, current user | Implemented and tested. No administrator account |
| macOS | Keychain generic password for `com.wind.agent` | Written, not compiled here |
| Linux | Secret Service on the session bus | Written, not compiled here |

Tracking services do not call the secret store. Auth tokens, refresh tokens, and cloud keys are not written to SQLite, configuration, or logs.

## Disk limit

The default limit is 1 GiB, stored in `metadata` on first open. Usage is the database, its WAL file, recorded artifact sizes, and orphan sizes. At 90 percent Wind logs `storage_disk_limit_reached` once. A write that would pass the limit is refused. Pending rows are not deleted to free space. There is no automatic retention of unsynchronized data.

## Logging

Storage logs initialization, open, artifact registration, write failure, the disk limit, integrity failure, and an unavailable secret store. Logs do not include secrets, raw URLs, SQL bound to user data, or file bytes.

## Intentionally not implemented

Backend synchronization, AWS or S3 upload, presigned URLs, authentication, token refresh, connectivity checks, remote deletion, remote configuration, and conflict resolution are not implemented. There is no HTTP client and no AWS SDK. Local interval timing lives in the supervisor.
