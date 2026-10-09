# Development

Run commands from the workspace root.

```bash
cargo fmt
cargo check --workspace
cargo test --workspace
cargo run -p wind-supervisor
```

`cargo test` uses the package directory as its working directory. Development configuration loading walks parent directories, so crate tests still find `config/product.toml`. Release builds embed that file and the logo instead of reading them from disk.

## Where new code goes

- Branding fields: `config/product.toml`, read through `wind-config`.
- Service IDs, the service trait, errors, and logging: `crates/core`.
- Registration, lifecycle, health, and shutdown: `crates/service-manager`.
- Host detection, session context, and runtime paths: `crates/platform`.
- Resident lifetime and shutdown coordination: `crates/supervisor`.
- A capability's behavior: that capability's crate.
- OS API bindings: `platforms/macos`, `platforms/windows`, or `platforms/linux`, called from the platform boundary.
- Wiring new services into startup: `crates/supervisor`.

## Current boundary

The service runtime can register, start, stop, restart, report health, and shut down. The supervisor stays resident until Ctrl+C or a termination signal, then stops services through that same shutdown path. Services declare capabilities, and the platform provider reports permission state without prompting. Screenshot capture is one explicit PNG. Video recording is one explicit H.264 MP4. Application tracking is an explicit foreground query. URL tracking reads the current browser address field when asked. Activity tracking counts input volume and does not store keys or coordinates. Starting Wind does not capture, record, or upload. Do not add permission prompts, authentication, networking, an updater, or an operating-system service installer until a later phase asks for them.

## Platform builds

`platforms/macos` and `platforms/linux` are excluded from workspace members so a Windows `cargo test --workspace` does not need Swift, pkg-config, or PipeWire. On each host, `wind-platform` depends on that host's provider, so the workspace command builds the provider for the machine you are on. `wind-windows` stays a workspace member, and its WinRT dependencies compile only when the target is Windows.

```text
Windows:
    cargo test --workspace

macOS (Rust 1.85+, Xcode Command Line Tools, macOS 15 SDK for video):
    cargo test --workspace
    cargo test --manifest-path platforms/macos/Cargo.toml
    scripts/package-macos-app.sh

`scripts/package-macos-app.sh` builds the release `wind` binary and wraps it in a `Wind.app` bundle. Release configuration and the logo are embedded in that binary. `WIND_RUNTIME_ROOT` selects an absolute home anchor so a test run does not write `~/Library`.

Linux (pkg-config, libxcb, libpipewire, clang; OpenH264 fetches Cisco's BSD binary at build):
    cargo test --workspace
    cargo test --manifest-path platforms/linux/Cargo.toml
```

Explicit commands, after the process is running:

```text
wind --capture-screenshot --count 2
wind --record-video --seconds 4
wind --current-application
wind --current-url
wind --activity-snapshot
```

The shipped `config/product.toml` leaves `video` disabled. Point `WIND_PRODUCT_CONFIG` at a copy that sets `[services.video] enabled = true` before `--record-video`. A normal `wind` start does not capture or record.

The runtime model is described in `docs/architecture/service-runtime.md`, `docs/architecture/platform-runtime.md`, `docs/architecture/capability-permissions.md`, `docs/architecture/screenshot-capability.md`, `docs/architecture/video-capability.md`, `docs/architecture/application-tracking.md`, `docs/architecture/url-tracking.md`, and `docs/architecture/activity-capability.md`.

`cargo run -p wind-supervisor` stays running. Stop it with Ctrl+C. The package name is `wind-supervisor`; the executable name is `wind`.
