# Platform runtime

The supervisor stays resident until the operating system asks it to stop. This layer resolves where the process is running and how it exits. It does not capture the screen, keyboard, applications, URLs, or video, and it does not install a login item or system service.

```text
Wind Supervisor
      |
      +-- Runtime context
      |
      +-- Platform runtime
      |      |
      |      +-- macOS
      |      +-- Windows
      |      +-- Linux
      |
      +-- Shutdown coordinator
      |
      +-- Runtime loop
      |
      +-- Service manager
             |
             +-- screenshot, activity, app_tracker, url_tracker, video
```

Capability services do not handle signals, choose process lifetime, discover paths, or own supervisor state. Permission questions go through the platform permission provider described in [capability-permissions.md](capability-permissions.md). That provider does not open settings during startup. Screenshot capture goes through the provider described in [screenshot-capability.md](screenshot-capability.md). Video recording goes through [video-capability.md](video-capability.md). Each operating system builds only its own provider.

## Startup

```text
Load configuration
        |
Load product identity
        |
Detect platform
        |
Resolve current user and session
        |
Resolve runtime paths
        |
Create runtime context
        |
Start service manager
        |
Enter resident runtime loop
```

`RuntimeContext` is built once. Later health checks and shutdown use that value. The context holds the loaded `ProductConfig`, `PlatformInfo`, `RuntimePaths`, `SessionContext`, the health interval, and the stop budget.

`product.identifier` is passed into path resolution. Platform code does not parse `product.toml` and does not hard-code the display name or `com.wind.agent`.

## Platform abstraction

`PlatformRuntime` is the only runtime surface the supervisor needs:

- `platform_info`
- `current_user`
- `session_context`
- `runtime_paths(product_identifier)`

`HostRuntime` is the implementation for the target being compiled. Operating-system differences stay inside `crates/platform`. The supervisor has no `cfg(target_os)`.

There is no install-state API. LaunchAgent, Windows Service, and systemd installation are a later packaging phase.

## Session

`SessionKind` is `Interactive`, `NonInteractive`, or `Unknown`.

| Kind | Meaning |
| --- | --- |
| `Interactive` | A user is present and the environment looks like a desktop session |
| `NonInteractive` | A user is present without a desktop session, for example a Linux tty |
| `Unknown` | No user name was available |

`UserContext` stores an optional username and an optional user id. Linux reads the process uid from `/proc/self/status`. macOS reads `id -u` without a shell. Windows leaves the user id empty. The runtime does not collect email, machine identifiers, files, credentials, or environment secrets.

On Windows and macOS, a resolved user is treated as `Interactive`. On Linux, a user is `Interactive` when `DISPLAY`, `WAYLAND_DISPLAY`, or a non-tty `XDG_SESSION_TYPE` is set. This does not bypass session isolation and does not request privacy permissions. A future screenshot service can refuse to run when the kind is not `Interactive`.

## Runtime paths

Paths are absolute, deterministic, and independent of the working directory. The product identifier is one directory component.

| Role | macOS | Windows | Linux |
| --- | --- | --- | --- |
| Config | `~/Library/Application Support/<id>/config` | `%APPDATA%\<id>\config` | `$XDG_CONFIG_HOME/<id>` or `~/.config/<id>` |
| Data | `~/Library/Application Support/<id>/data` | `%LOCALAPPDATA%\<id>\data` | `$XDG_DATA_HOME/<id>` or `~/.local/share/<id>` |
| Cache | `~/Library/Caches/<id>` | `%LOCALAPPDATA%\<id>\cache` | `$XDG_CACHE_HOME/<id>` or `~/.cache/<id>` |
| Logs | `~/Library/Logs/<id>` | `%LOCALAPPDATA%\<id>\logs` | `$XDG_STATE_HOME/<id>/log` or `~/.local/state/<id>/log` |
| Runtime | `~/Library/Application Support/<id>/runtime` | `%LOCALAPPDATA%\<id>\runtime` | `$XDG_RUNTIME_DIR/<id>` when that directory is absolute, otherwise `~/.local/state/<id>/runtime` |

Relative XDG values are ignored. The Linux runtime directory does not fall back to `/run`, because that location can require root. Windows falls back from `APPDATA` / `LOCALAPPDATA` to the user profile's `AppData` directories. Protected system directories are not used. When `WIND_RUNTIME_ROOT` is an absolute directory, Windows uses that directory in place of both profile roots. An unset value keeps the normal profile paths.

Startup creates these directories for the current user. It does not write `./config`, `./logs`, or `./data`.

## Runtime state

Supervisor state is separate from the service lifecycle owned by `wind-service-manager`.

```text
Initializing → Starting → Running
Running → ShutdownRequested → Stopping → Stopped
```

`Failed` is available for a runtime failure. Invalid transitions are rejected and leave the current state unchanged. A service that fails to start does not by itself move the supervisor to `Failed`; that service stays `Failed` and the process remains `Running` until shutdown.

## Shutdown

```text
Ctrl+C, SIGINT, SIGTERM, Windows console termination, internal request
        |
Shutdown coordinator
        |
ServiceManager.shutdown()
        |
reverse registration order, existing stop budget
        |
process exits
```

`ShutdownCoordinator` is the only gate. `request` is idempotent: the first call wakes the runtime thread, and later calls only increment a duplicate count. The runtime thread performs cleanup. A second `Session::shutdown` after `Stopped` does not call service stop again.

The wait uses a condition variable. The loop sleeps until shutdown or until `health_check_interval_seconds` elapses. It does not poll in a tight loop.

Health polling reads `ServiceManager::health_all`, logs `health_check`, and logs `service_degraded` or `service_unhealthy` when those states appear. It does not restart services and it does not print health on stdout.

## Signals

The workspace forbids `unsafe` code, and the standard library cannot install a portable `SIGTERM` or Windows console handler. `wind-platform` depends on `ctrlc` for that notification only.

`ctrlc` delivers the callback on its own thread. The callback calls `ShutdownCoordinator::request` and returns. On Unix the events are `SIGINT` and `SIGTERM`. On Windows they are console control events, including Ctrl+C. This is not a Service Control Manager handler.

Tests do not install the process-wide handler. They request shutdown through the coordinator directly.

## Why OS service installation is deferred

A correct resident process has to exist before an installer decides how the operating system starts it. Installing a LaunchAgent, a Windows service, or a systemd unit now would mix packaging, privileges, and persistence into the runtime. The process runs as the current user. A later packaging phase can start this same supervisor; it should not need a second shutdown implementation.

## Later process isolation

Services remain in-process. `ExecutionMode::OutOfProcess` is still metadata. A future process-backed service can implement the same `Service` trait. The supervisor would still start, wait, and stop through the service manager. That handle is not implemented here.
