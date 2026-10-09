# Architecture

The workspace, branding, service runtime, resident platform runtime, and permission model are in place. Screenshot, video, application, URL, and aggregate activity capabilities are implemented. Finalized tracking records are stored locally. Installers, authentication, and upload are not.

## System architecture

```text
                         Supervisor
                             |
       +---------------------+---------------------+
       |                     |                     |
  Screenshot            Activity             App Tracker
    Service              Service                Service
       |                     |                     |
       +---------------------+---------------------+
                             |
                        URL Tracker
                             |
                        Video Service
```

The supervisor loads configuration, resolves the platform runtime once, and drives the service runtime. It stays resident until shutdown. Lifecycle, health, and service shutdown live in the service manager. The supervisor can launch the application tracker as its own executable when a local manifest says so. The other services stay in-process. Finalized records go through one local store. See [local-storage.md](local-storage.md). Authentication, permission prompts, and a network updater are still later work. See [service-runtime.md](service-runtime.md), [service-ipc.md](service-ipc.md), [service-versioning.md](service-versioning.md), and [service-updates.md](service-updates.md).

## Supervisor

`crates/supervisor` is the composition root and the only executable. It depends on configuration, platform detection, the service registry, and the capability crates. Those crates do not depend on the supervisor.

Startup order:

1. Load `config/product.toml` through `wind-config`.
2. Detect the platform, current user, and session.
3. Resolve absolute user-scoped runtime directories from `product.identifier`.
4. Register `screenshot`, `activity`, `app_tracker`, `url_tracker`, and `video`.
5. Start enabled services sequentially.
6. Write the runtime report to stdout and wait for termination.
7. Stop running services in reverse registration order and exit.

## Service model

`wind-core` defines service identity, the lifecycle state enum, health, readiness, and the `Service` trait: `id`, `name`, `version`, `execution_mode`, `start`, `stop`, `health`, `readiness`, and `configuration`.

`wind-service-manager` is the registry and the state machine. It rejects a duplicate ID, preserves registration order, and is the only component that moves a service between states. It does not install system services or spawn processes.

`screenshot` captures one full display when `capture_once` is called. `video` records when `start_recording` is called and stays disabled in the shipped configuration. `app_tracker` reports the foreground application when asked. `url_tracker` reads the current browser address field when asked. `activity` counts keyboard and mouse volume and reports it when `activity_snapshot` is called. Every service leaves `monitoring_enabled` false. See [screenshot-capability.md](screenshot-capability.md), [video-capability.md](video-capability.md), [application-tracking.md](application-tracking.md), [url-tracking.md](url-tracking.md), and [activity-capability.md](activity-capability.md).

Service enablement comes from `[services.<id>]` in `product.toml`. A missing table defaults to enabled. `video` ships disabled so `Disabled` stays distinct from `Failed`.

## Configuration

```text
product.toml -> wind-config -> supervisor
```

`ProductConfig`, `ProductIdentity`, `BrandingConfig`, `CompanyConfig`, and `RuntimeConfig` are the typed forms. Validation rejects empty fields, malformed identifiers, bad versions, absolute logo paths, `..` in the logo path, and a zero runtime interval. Loading a file also checks that the logo exists.

No other crate reads TOML. Service enable flags are part of the same file and the same loader.

## Product identity

`product.identifier` is the stable product ID, initially `com.wind.agent`.

Derived technical IDs:

```text
com.wind.agent.supervisor
com.wind.agent.screenshot
com.wind.agent.activity
com.wind.agent.app-tracker
com.wind.agent.url-tracker
com.wind.agent.video
```

The suffix list lives in `wind-core`. The product identifier lives in `product.toml`. Neither includes `display_name`.

## Branding

`display_name`, `product.name`, `company.name`, and `branding.logo` are branding. The supervisor report uses `display_name`. Logs include the branding fields so they are actually loaded.

The logo file is `assets/branding/logo.svg`. Future UI, installers, notifications, and documentation should use the configured path rather than a hard-coded asset location.

## Platform abstraction

`wind-platform` returns host facts, a `SessionContext`, and `RuntimePaths`. `PlatformRuntime` is the trait the supervisor calls. The host implementation is `HostRuntime`.

- `operating_system`: macOS, Windows, Linux, or another Rust target name
- `architecture`: `x86_64`, `aarch64`, or another Rust target name
- `platform_version`: host version when the platform crate can read it
- `session`: `Interactive`, `NonInteractive`, or `Unknown`, plus an optional username and user id

Conditional compilation stays inside `crates/platform`. Callers match on `OperatingSystem`; they do not use `cfg`.

Runtime directories are absolute and user-scoped. The directory name is `product.identifier`, not `display_name`. The process working directory is not used. Path layouts, shutdown signals, and the reason OS service installation is deferred are described in [platform-runtime.md](platform-runtime.md).

`platforms/macos` contains the ScreenCaptureKit screenshot provider. `platforms/windows` and `platforms/linux` do not implement capture. Windows Graphics Capture, X11, Wayland, and systemd are still absent.

## Service versioning

Each service crate sets its own Cargo package version. `create()` parses `CARGO_PKG_VERSION`. The product version in `product.toml` is separate and is the version shown in the startup report.

A later release manifest can ship `screenshot` 2.3.0 while `activity` stays 1.4.0 and `video` stays 1.1.0. Phase 1 only makes those versions representable and printable.

## Security boundaries

- The supervisor composes services. It does not grant itself every future capability.
- Each service will eventually receive only the permission its feature needs.
- Secrets and tokens are not part of this design and must not be hard-coded.
- Platform detection reads the process environment and public version information. It does not query privacy databases.
- Placeholders cannot collect monitoring data because they do not implement collection.
- Production errors carry component and operation context. Logging is local.

The full rule list is in `AGENTS.md`.

## Capabilities and permissions

Each capability service declares the capability it requires. The platform permission provider reports permission state. The service manager turns that into capability status and health. A missing permission leaves the service `Running` and sets health to `PermissionRequired`. It does not mark the service `Failed` and it does not stop the supervisor.

The provider does not grant, bypass, or request permission. macOS screen capture and video readiness use `CGPreflightScreenCaptureAccess` and do not prompt. Windows capture stays `Unknown` at the permission layer and reports availability from the capture provider. See [capability-permissions.md](capability-permissions.md) and [screenshot-capability.md](screenshot-capability.md).

## Future IPC

Supervisor-to-service communication is not implemented. No transport is selected.

The supervisor already calls `start`, `stop`, `restart`, `health`, and status through the service trait. A later out-of-process handle can implement that trait. The transport still has to be chosen from the real permission and process boundaries, on macOS, Windows, and Linux.

## Future updates

Not implemented. The intended flow is a release manifest the supervisor checks per component:

```text
                    Supervisor
                        |
                   Release API
                        |
           +------------+------------+
           |            |            |
      Screenshot     Activity      Video
       v2.1.0         v1.4.0       v1.2.0
```

The supervisor will eventually:

1. Check the release manifest.
2. Decide which component is older than the manifest.
3. Download that component.
4. Verify authenticity.
5. Stage it.
6. Stop the affected component.
7. Replace it.
8. Restart it.
9. Check its health.
10. Roll back if the health check fails.

Signing and verification are required before an update can be installed. Phase 1 has no downloader, manifest client, or package layout.
