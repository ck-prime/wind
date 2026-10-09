# Activity capability

The activity service answers one question: how much keyboard and mouse interaction has happened since this provider started. It does not answer which keys were pressed or where the pointer moved.

```text
Native input event
    |
    v
Platform listener classifies the message id
    |
    v
ActivityCounters
    |
    v
ActivitySnapshot
```

The service, supervisor, log, and storage see only the snapshot. A keyboard event becomes `record_keyboard_event()`. A movement becomes `record_mouse_move()`. Those functions take no key, character, scan code, or coordinate.

`input_monitoring` and the service id `activity` are unchanged. `[services.activity] enabled = true` is the only switch. There is no sampling interval in `product.toml`. `monitoring_enabled` stays false. Nothing is written under `<data_dir>/activity`. A non-zero counter delta is stored in the local database when a snapshot is taken or the service stops. See [local-storage.md](local-storage.md).

## Counters

| Field | Meaning |
| --- | --- |
| `keyboard_events` | Key-down messages. Key-up is not counted. Auto-repeat is another key-down when the platform delivers one. |
| `mouse_moves` | Pointer-move messages. Coordinates are discarded. |
| `mouse_clicks` | Button-down messages. Button-up is not counted, so one press is one click. |
| `mouse_scrolls` | One wheel or scroll message. The wheel delta is discarded. |

Counters are `u64` and saturate at `u64::MAX`. `snapshot()` returns the cumulative totals and the current UTC `SystemTime`. It does not reset them. `delta_since(previous)` subtracts an earlier snapshot without changing either one. A new provider instance starts at zero. Stopping and starting the same instance keeps the totals.

## Health

| Provider state | Lifecycle | Health |
| --- | --- | --- |
| Listener running | `Running` | `Healthy` |
| Permission missing | `Running` | `PermissionRequired` |
| Platform cannot observe global input | `Running` | `Unsupported` |
| Listener could not start | `Running` | `Degraded` |
| Snapshot fails after a successful start | `Running` | `Unhealthy` |

A missing permission or an unsupported session does not fail the service and does not stop the supervisor.

## Platforms

Windows uses `WH_KEYBOARD_LL` and `WH_MOUSE_LL` on one thread named `wind-activity-input`. The callback reads the message id only and then calls `CallNextHookEx`. It does not read the hook structure. The hooks run in this process, so no DLL is injected. No driver, administrator account, or consent prompt is required. Windows exposes no permission query for this mechanism, so the permission state stays `Unknown`. The provider reports `Available` once the hooks are installed.

macOS uses a session event tap in listen-only mode. `CGPreflightListenEventAccess` reports whether Input Monitoring is granted and does not prompt. `CGRequestListenEventAccess` is not called. A false preflight result is `NotDetermined` at the permission layer and `PermissionRequired` for the service. The callback reads the event type and returns the event unchanged.

Linux X11 selects XInput2 raw key, motion, and button events on the master devices. The button number is used only to treat buttons 4 through 7 as scrolls, then discarded. Wayland without `DISPLAY` is `Unsupported`. A session with neither `DISPLAY` nor `WAYLAND_DISPLAY` is `Unavailable`. When both are set, the X11 path is used and sees X11 clients, including XWayland clients, not every native Wayland client. The process does not need root.

## Logging and shutdown

Lifecycle logs are `activity_listener_started`, `activity_listener_stopped`, `activity_permission_required`, `activity_provider_unavailable`, and `activity_provider_failed`. They do not include key names, scan codes, characters, coordinates, window titles, or URLs. The explicit `wind --activity-snapshot` command prints the four counters for the operator who asked.

Shutdown sets a stop flag, posts `WM_QUIT` on Windows or leaves the X11 poll on Linux, unhooks or drops the display connection, and joins the listener thread. Dropping the provider does the same.
