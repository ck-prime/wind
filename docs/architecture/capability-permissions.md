# Capabilities and permissions

The permission contract sits between a service and the operating system. Screenshot, video, application, URL, and aggregate activity capabilities use it. Nothing here grants or bypasses a permission. Activity counts do not include key identity or pointer coordinates.

```text
Service
    |
Capability requirement
    |
Permission provider
    |
+---+---+---+
macOS Windows Linux
```

Three layers stay independent:

| Layer | Examples | Owner |
| --- | --- | --- |
| Service lifecycle | `Running`, `Failed`, `Disabled` | Service manager |
| Capability status | `Available`, `PermissionRequired`, `Unsupported`, `Unavailable`, `Unknown` | Permission provider plus session adjustment |
| Permission state | `Granted`, `Denied`, `NotDetermined`, `Restricted`, `Unsupported`, `Unknown` | Permission provider |

A missing permission is not a service crash and does not stop the supervisor.

## Capabilities

Stable identifiers live in `wind-core`:

```text
screen_capture
input_monitoring
active_application
browser_url
video_capture
```

Each capability crate declares its own requirement. The supervisor does not contain that mapping.

| Service | Capability | Required |
| --- | --- | --- |
| `screenshot` | `screen_capture` | yes |
| `activity` | `input_monitoring` | yes |
| `app_tracker` | `active_application` | yes |
| `url_tracker` | `browser_url` | yes |
| `video` | `video_capture` | yes |

`required: false` is supported. An optional capability that is not available sets health to `Degraded`. No current service uses an optional capability.

`monitoring_enabled` stays false. A granted permission does not start a capture schedule. Screenshot bytes are produced only by an explicit `capture_once` call.

## Permission provider

`PermissionProvider` has two operations:

- `check(capability)` returns `PermissionStatus`
- `open_settings(capability)` opens a supported settings destination when one exists

There is no grant, approve, bypass, or elevate method. `open_settings` is not called during startup or health polling. `Ok(())` means the settings surface opened. It does not mean the permission was granted. The caller checks again. Calling it more than once only opens the same surface again.

```text
Capability
    |
Permission check
    |
Permission required
    |
Open OS Settings
    |
User grants permission
    |
Wind re-checks
    |
Service becomes healthy
```

Wind never grants a permission and never bypasses an operating-system control. Windows does not use the macOS privacy model. Linux depends on the desktop and session. Settings navigation is best-effort and platform-specific. Permission state is re-checked rather than assumed.

`capability_status` defaults to the permission mapping below. The host provider overrides it when the session cannot support a desktop capability.

| Permission state | Capability status |
| --- | --- |
| `Granted` | `Available` |
| `Denied`, `NotDetermined`, `Restricted` | `PermissionRequired` |
| `Unsupported` | `Unsupported` |
| `Unknown` | `Unknown` |

A non-interactive session changes `Unknown` or `Available` to `Unavailable` for screen capture, input monitoring, active application, and video capture. Browser URL is read from the foreground accessibility tree when a query runs. That capability is not adjusted by the session flag. A denial stays `PermissionRequired`. Windows UI Automation has no consent query, so its permission answer stays `Unknown`. macOS Accessibility permission is reported by the URL provider and is not requested.

Health for a running service:

| Capability status | Required | Optional |
| --- | --- | --- |
| `Available` | keep service health, normally `Healthy` | same |
| `Unknown` | keep service health | same |
| `PermissionRequired` | `PermissionRequired` | `Degraded` |
| `Unsupported` | `Unsupported` | `Degraded` |
| `Unavailable` | `Degraded` | `Degraded` |

`Unhealthy` from the service itself still wins. The lifecycle stays `Running` unless `start` or `stop` actually fails. Disabled services are not checked.

The service manager evaluates this on every `health` and `capability_reports` call. Nothing is cached, and there is no operating-system permission watcher. The existing health interval is the refresh point. Startup logs one `capability_checked` line per running service. The stdout report stays the short lifecycle line and does not repeat permission state.

## Platform answers in this phase

These answers are static and truthful. They do not call privacy databases.

### macOS

Screen capture on a macOS build uses `CGPreflightScreenCaptureAccess`. Input monitoring on a macOS build uses `CGPreflightListenEventAccess`. Granted means the boolean is true. Not granted is `NotDetermined`, because those functions cannot distinguish a first prompt from an explicit denial. They do not prompt and they do not call the request APIs. Video capture stays `Unknown` until a recording is requested. Active application is `Granted` because foreground identity does not use a separate privacy permission. Browser URL stays `Unknown` until the URL provider reports Accessibility access. TCC is not read or modified.

An explicit settings request opens System Settings at the matching privacy pane:

| Capability | Destination |
| --- | --- |
| `screen_capture`, `video_capture` | Privacy & Security → Screen & System Audio Recording |
| `input_monitoring` | Privacy & Security → Input Monitoring |
| `browser_url` | Privacy & Security → Accessibility |
| `active_application` | no settings page; the result is `Unsupported` |

The destination uses the established `x-apple.systempreferences` privacy anchors. Older macOS versions may ignore an anchor and open the Security pane instead. The process is `open` with that fixed URL. No shell runs, and the code does not click Settings. This Windows workspace does not compile or run the macOS launcher.

### Windows

Every capability stays `Unknown`. Windows has no separate screen-capture, video-capture, or input-monitoring consent query, so the provider does not report `Granted` and does not offer an Open Settings action. UI Automation has no user-grantable browser permission. If a security boundary blocks a window, the URL provider reports that failure and does not elevate. The capture crates report whether a display can actually be captured. The activity provider reports whether the low-level hooks installed. The provider does not elevate, change UAC, modify registry permissions, or install a service.

### Linux

Desktop capabilities stay `Unknown` until a non-interactive session makes them `Unavailable`. Screen capture and video capture are usable only when the platform provider can use X11 or an explicit portal capture. Activity on X11 uses XInput2 raw events. Wayland without `DISPLAY` is unsupported for global input. A health check does not open a portal. Browser URL stays `Unknown` on a non-interactive session. The process does not need root and does not install systemd.

`open_settings` does not spawn `xdg-open` and does not open a portal dialog. A headless session returns `Unavailable`. An X11 or Wayland session returns `Unsupported` with an explanation for that environment. There is no single Linux settings page for these capabilities.

## What is deferred

Screenshot capture, video recording, application queries, URL observations, and activity snapshots are explicit. Activity has no sampling interval. Scheduling, permission prompts, and MDM profiles come later. Opening Settings is not a permission prompt. Configuration cannot grant an operating-system permission, so `product.toml` has no permission flags. Service `enabled` remains the only on/off switch.

`wind --permissions` prints the current check. `wind --open-permission CAPABILITY` opens a real settings surface when one exists, then checks again. On Windows both commands report `Unknown` permission and an unsupported settings result.
