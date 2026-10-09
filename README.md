# Wind

Wind is a cross-platform desktop agent. The supervisor loads product configuration, starts enabled in-process services, and stays resident until Ctrl+C or a termination signal. The screenshot service captures the primary display when asked. The video service records it when asked. The application tracker reports the foreground application when asked. The URL tracker reads the current browser address field when asked. The activity service counts keyboard and mouse volume while it is running and reports that volume when asked. Starting the process does not capture, record, or upload.

Supported platforms are macOS, Windows, and Linux. One workspace contains the shared code and the platform boundary. There are not separate products per operating system.

## Architecture

The supervisor is the composition root. It loads `config/product.toml`, detects the host, and drives `wind-service-manager`. Capability crates depend on shared abstractions. Operating-system calls stay behind `crates/platform` and, later, `platforms/`.

```text
config/product.toml
        |
        v
   wind-config
        |
        v
   supervisor -------- wind-platform
        |
        +---- service-manager
        |
        +---- screenshot, activity, app-tracker, url-tracker, video
                        |
                        v
                     wind-core
```

Service IDs and the product identifier stay stable when the visible product name changes. Branding comes from configuration, not from Rust string literals.

See [docs/architecture/overview.md](docs/architecture/overview.md) for the system shape and [docs/architecture/service-runtime.md](docs/architecture/service-runtime.md) for lifecycle, health, and the process model.

## Repository structure

```text
config/                 product.toml, the branding source of truth
assets/branding/        logo referenced by product.toml
crates/core             service IDs, versions, errors, logging
crates/config           the only parser of product.toml
crates/platform         OS, session, runtime paths, and termination signals
crates/service-manager  registry, lifecycle, and shutdown
crates/screenshot       screenshot capture service
crates/activity         aggregate keyboard and mouse counts
crates/app-tracker      foreground application tracker
crates/url-tracker      browser URL tracker
crates/video            explicit video recording service
crates/supervisor       supervisor executable
platforms/macos         ScreenCaptureKit screenshot and video provider
platforms/windows       Windows Graphics Capture and Media Foundation
platforms/linux         X11, portal screencast, and OpenH264 video
docs/                   architecture and development notes
scripts/                reserved for future development scripts
tests/                  where the crate tests live
```

## Workspace

```bash
cargo check --workspace
cargo test --workspace
cargo run -p wind-supervisor
```

Each crate has its own `version` in its `Cargo.toml`. Service versions are read from those package versions at runtime. They are independent of `product.version` in `config/product.toml`, which is the version printed for the product.

The executable name is `wind`. That command name is stable technical identity. The visible name is `product.display_name`.

## Configuration and branding

`config/product.toml` holds the product name, display name, technical identifier, product version, logo path, company name, per-service `enabled` flags, and the resident runtime intervals. The supervisor loads it through `wind-config`. A missing `[services.<id>]` table defaults to `enabled = true`. The shipped file sets `video` to `enabled = false`. A missing `[runtime]` field defaults to a 10 second health check and a 5 second service stop budget.

To rename what the application shows, change `display_name` and run the supervisor again. No Rust change is required:

```toml
display_name = "My Product"
```

The startup report then begins with `My Product Agent`. `product.identifier` and the service IDs stay the same.

The logo is [assets/branding/logo.svg](assets/branding/logo.svg). It is an original mark: a teal tile with three wind strokes. It is pure SVG, with no embedded images, and the tile keeps the mark visible on light and dark backgrounds.

`WIND_PRODUCT_CONFIG` can point at a different product file in development builds. Otherwise a development build walks upward from the working directory, the executable, and the config crate manifest looking for `config/product.toml`. Release builds embed that file and `assets/branding/logo.svg` and do not read them from disk.

## Services

| Service ID | Technical suffix | Crate |
| --- | --- | --- |
| `screenshot` | `screenshot` | `wind-screenshot` |
| `activity` | `activity` | `wind-activity` |
| `app_tracker` | `app-tracker` | `wind-app-tracker` |
| `url_tracker` | `url-tracker` | `wind-url-tracker` |
| `video` | `video` | `wind-video` |

The supervisor's technical suffix is `supervisor`. A full identifier is `{product.identifier}.{suffix}`, for example `com.wind.agent.screenshot`.

The supervisor starts enabled services, leaves disabled services in `Disabled`, and waits. `monitoring_enabled` stays false. A screenshot is taken only by an explicit capture request, not at startup. Health checks are written to stderr on the configured interval. They do not restart services and they are not printed on the runtime report.

Structured logs go to stderr and include timestamp, level, component, service id, version, platform, state, message, and error. The runtime report goes to stdout. Nothing is sent to a remote log service. Runtime directories are absolute paths under the current user's application directories, named with `product.identifier`.

## Runtime scope

Included:

- Cargo workspace and crate boundaries
- Product configuration, branding, and service enablement
- Supervisor runtime report and resident process
- In-process service registry, lifecycle, health, and shutdown
- Platform detection, user session, and user-scoped runtime paths
- Graceful Ctrl+C and termination-signal shutdown
- Capability declarations and permission-aware health
- Explicit primary-display screenshot capture and H.264 video recording
- Local structured logging and contextual errors
- Tests for configuration, registration, lifecycle, shutdown, screenshot, and video behavior

Not included:

- Raw key logging, pointer coordinates, or activity history files
- Screenshot or video scheduling or upload
- Automatic permission prompts or settings windows
- Installers, launch agents, or system services
- Authentication and secrets
- Networking, upload, or remote logging
- IPC between supervisor and services
- Updates, signing, or rollback
- Desktop UI

## Development rules

- Keep the product display name in `config/product.toml`.
- Parse that file only in `wind-config`.
- Keep service IDs and technical suffixes in `wind-core`.
- Keep operating-system calls in `crates/platform` or, later, `platforms/`.
- Do not add a dependency unless the workspace needs it.
- Production paths return errors with component, operation, and cause. Do not use `unwrap`, `expect`, or `panic` there.
- Follow the security rules in [AGENTS.md](AGENTS.md).
