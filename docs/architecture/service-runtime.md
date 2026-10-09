# Service runtime

The service manager owns lifecycle state. Services do not own the supervisor, and they do not start one another. Built-in services run in the supervisor process unless a local manifest selects an out-of-process executable.

```text
Supervisor
    |
Service Manager
    |
In-process services. The supervisor closes a 10-second tracking interval and asks for one screenshot. Video recording stays explicit. Application and URL identity are read at each interval boundary. Activity keeps aggregate counters while its listener runs.
```

The supervisor is a foreground process. It stays resident until termination and does not install a LaunchAgent, a Windows service, or a systemd unit. Platform paths, session facts, and the shutdown signal path are described in [platform-runtime.md](platform-runtime.md).

## Startup

Startup is sequential, in registration order:

```text
Supervisor
    |
Configuration
    |
Platform detection
    |
Service manager
    |
Service registration
    |
Service startup
    |
Resident wait until shutdown
```

Sequential startup keeps logs and failures ordered. Five placeholders do not need concurrent start. A later phase can add concurrency if a measured startup time requires it.

There is no dependency solver. A future service may declare dependencies, for example video depending on a capture runtime. That declaration does not exist yet. Registration order is the only order.

## Shutdown

```text
Supervisor shutdown
    |
stop running services, reverse registration order
    |
wait for each stop to return
    |
exit
```

Reverse order is the conservative pair to sequential startup: the last service started is the first service stopped. The stop budget is `[runtime] shutdown_timeout_seconds` in `product.toml`. The shipped value is 5 seconds. `RuntimeSettings` keeps the same 5 second default for callers that do not load product configuration. The supervisor passes the configured value into the service manager.

Stops are synchronous. The manager records an error when a stop returns after the budget. It does not kill the process or spawn a watchdog. Ctrl+C, `SIGINT`, `SIGTERM`, and Windows console control events only notify `ShutdownCoordinator`. The runtime thread then calls `Session::shutdown`, which calls `ServiceManager::shutdown`. A repeated request does not stop services twice.

Disabled, stopped, and failed services are not stopped again.

A resident supervisor also finalizes the open tracking interval before it stops services. That row ends at the actual shutdown time when shutdown falls inside the window. The next process starts a new session.

## State machine

```text
Registered → Starting → Running → Stopping → Stopped
                |          |
                v          v
              Failed     Failed

Failed → Restarting → Starting
Stopped → Restarting → Starting
Stopped → Starting

Registered → Disabled
Stopped → Disabled

Starting → Failed
Stopping → Failed
Restarting → Failed
```

`Disabled` is configuration. `Failed` is a start or stop error. They are different states. A disabled service is never started. A failed service stays `Failed` until `restart`. Calling `start` on `Failed` is rejected.

Invalid transitions return an error and leave the current state in place.

Restart of a running service stops it, then passes through `Restarting` and `Starting`. If stop fails, restart stops there.

## Failure policy

`start_all` continues after a failure. The failed service becomes `Failed`, the error is kept, and the next enabled service still starts. Shutdown still runs. The process exits with status 1 when any start or shutdown error was recorded. Standard output still shows the runtime report.

## Health and readiness

Health is not derived from lifecycle state, so `Running` and `Unhealthy` can occur together. For a running service that declares capabilities, the manager also folds in the current permission result. `PermissionRequired` and `Unsupported` are health states, not lifecycle failures. An unknown permission leaves a placeholder's own health in place. Screenshot and video report health from their provider status. An unknown operating-system consent result does not replace an available provider. The check runs again on each health poll and is not cached.

| Lifecycle | Placeholder health |
| --- | --- |
| Not started, including `Disabled` | `Unknown` |
| Started | `Healthy` |

`Readiness` stays separate from lifecycle. A running service becomes `PermissionRequired` when a required capability is not granted, and `Ready` only when the provider reports that capability as available. Placeholders still leave `monitoring_enabled` false. The service manager does not query TCC or open a settings UI. The permission contract is described in [capability-permissions.md](capability-permissions.md).

## Process model

Each service exposes `ExecutionMode::InProcess` or `ExecutionMode::OutOfProcess`. The manager calls the same `start`, `stop`, `health`, and `maintenance` methods either way. It does not spawn processes, run a shell, or choose an executable.

```text
Supervisor
    |
Service Manager
    |
Service handle
    |
+---------------------------+------------------------------+
| In-process service        | Out-of-process handle        |
| screenshot, activity,     | app tracker, when a manifest |
| url tracker, video        | points at wind-app-tracker   |
+---------------------------+------------------------------+
```

Without `services.toml`, every service stays in-process. `WIND_SERVICE_MANIFEST` or `<runtime>/services.toml` can select the application tracker executable. The product identifier stays `com.wind.agent`. A service version is not part of that identity. See [service-ipc.md](service-ipc.md), [service-versioning.md](service-versioning.md), and [service-updates.md](service-updates.md).

The executable path must be absolute and its file name must be the platform service name, such as `wind-app-tracker.exe` on Windows. The supervisor does not start a shell.

## Crash recovery

`maintenance` runs from the resident loop, not from the health poll. An unexpected exit is restarted at most three times, with a short backoff. The next crash moves that service to `Failed`. The supervisor and the other services keep running. Shutdown still asks the process to exit, waits up to two seconds, and only then terminates it.

Process state (`Running`, `Crashed`, `Updating`, `Rollback`) stays separate from service lifecycle, health, and permission state.

## What this runtime does not do

No plugin loader, network updater, installer, or Windows service. The local update manager can stage, verify, activate, and roll back a package. It does not download one. Screenshot, activity, URL, and video capture still run in the supervisor process.
