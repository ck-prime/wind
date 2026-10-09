# Screenshot capability

Phase 5 adds one capture primitive. It does not schedule captures, upload them, or implement any other monitoring service.

```text
Screenshot service
    |
    +-- capability check
    |
    +-- storage under <data_dir>/screenshots
    |
    +-- ScreenshotProvider
            |
            +-- macOS: ScreenCaptureKit, one full display
            +-- Windows: Windows Graphics Capture, primary display
            +-- Linux: X11, or a portal screencast when capture is requested
```

The supervisor does not mention ScreenCaptureKit, image buffers, or PNG. The screenshot service does not mention TCC, Windows capture APIs, or a Linux display protocol.

## Request and result

The only request is full-display capture. There is no window, region, cursor, annotation, or OCR option.

The image is PNG. PNG bytes stay in `<data_dir>/screenshots`. After a successful write, the local database stores the path, size, and SHA-256. It does not store the image. See [local-storage.md](local-storage.md). The macOS provider encodes RGBA pixels with the `png` crate and drops the pixel buffer before returning. The service then writes those bytes and drops them.

`capture_once` is the only trigger. Starting the service does not capture. There is no interval.

## Permission

The screenshot service requires `screen_capture`.

| Provider status | Service health | Lifecycle |
| --- | --- | --- |
| Available | Healthy, unless the last capture or store failed | Running |
| PermissionRequired | PermissionRequired | Running |
| Unsupported | Unsupported | Running |
| Unavailable | Degraded | Running |
| Unknown | Unknown | Running |
| Capture failed while available | Unhealthy | Running |
| Storage or encoding failed | Degraded | Running |
| Worker failed to start | start returns an error | Failed |

Unknown permission is not treated as healthy for this service. A missing permission does not stop the supervisor.

On macOS, readiness uses `CGPreflightScreenCaptureAccess`. That public function does not prompt. `true` is `Granted`. `false` is `NotDetermined` because the function cannot tell a first prompt from an explicit denial. `CGRequestScreenCaptureAccess` is not called. TCC is not read. System Settings is not opened. No entitlement file is added: Screen Recording for this API is a user TCC grant, not an Accessibility, camera, microphone, or Full Disk Access entitlement.

A capture calls ScreenCaptureKit only after preflight reports access. `SCShareableContent` and `SCScreenshotManager` are released when the call returns. No stream is kept.

Windows has no separate screen-capture consent query. The permission state stays `Unknown`, and the provider reports `Available` when a display can be enumerated. A runtime access failure is classified from the capture error. Linux reports `Available` only after a capture has succeeded, or when an X11 display is present and a monitor can be listed. A Wayland session stays `PermissionRequired` until an explicit capture is accepted. A headless session is `Unavailable`. Neither platform reports `Granted`.

## Storage

Files go in `<data_dir>/screenshots`, using the runtime data directory. They are not written to the working directory, Desktop, Downloads, or a shared temporary directory. The directory is created on the first successful store.

Names are UTC `YYYYMMDD-HHMMSS-mmm.png`, with a numeric suffix when that name already exists. An existing file is not replaced. Names do not include a machine identifier.

On Unix the directory is mode `0700` and each file is created mode `0600`. Logs record width, height, and byte length. They do not record pixels or the file name.

## Concurrency and shutdown

One worker thread, named `wind-screenshot`, runs captures. A second `capture_once` while one is active returns `AlreadyInProgress`. `stop` stops accepting captures, lets the current capture finish, then joins the worker. The service manager's stop timeout still wraps that `stop` call.

## Dependencies

- `screencapturekit` 11, feature `macos_15_0`, only on macOS. It covers one-shot capture and `SCRecordingOutput`. A macOS build needs Rust 1.85 or newer and the Swift toolchain.
- `xcap` 0.9.8, with the Windows Graphics Capture feature on Windows and without it on Linux.
- `png` 0.17 in each platform crate, to encode one RGBA buffer as PNG.

No async runtime was added. `platforms/macos` and `platforms/linux` stay out of the workspace members so a Windows `cargo test --workspace` does not need Swift or Linux headers. On those hosts, `cargo test --workspace` builds the matching provider through the target-specific dependency.

## Deferred

Scheduling, upload, encryption, and retention cleanup are later work. Activity, application, and URL services are still placeholders. Video recording is described in [video-capability.md](video-capability.md).
