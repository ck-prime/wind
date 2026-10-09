# Tests

Unit tests live in the crate they cover. `cargo test --workspace` runs them.

- `crates/config`: product identity, service enablement, runtime intervals, defaults, and invalid service tables
- `crates/service-manager`: registry order, lifecycle, failed start, shutdown order, health, and permission-aware status
- `crates/supervisor`: enabled services start, capability declarations, resident shutdown is idempotent, health polling does not change lifecycle state, disabled services stay disabled
- `crates/platform`: operating system, session classification, absolute runtime paths, and permission answers that do not open settings
- `crates/screenshot`: fake capture outcomes, permission versus failure, storage names, and one-capture-at-a-time behavior
- `crates/activity`: aggregate counters, cumulative snapshots, saturation, permission and failure health, and shutdown
- `crates/app-tracker`: foreground identity, deduplication, unsupported and unavailable results, and shutdown
- `crates/url-tracker`: URL sanitization, native messages, deduplication, disconnect, and shutdown
- `crates/core`: stable service IDs, capability and permission states, state transitions, version parsing, and log fields
