# Video capability

Video is one explicit recording. It is not a sequence of screenshots, and it does not start when the service starts.

```text
Video service
    |
    +-- recording state, separate from the service lifecycle
    |
    +-- storage under <data_dir>/videos
    |
    +-- VideoProvider
            |
            +-- Windows: Windows Graphics Capture, Media Foundation H.264 MP4
            +-- macOS: ScreenCaptureKit stream, SCRecordingOutput H.264 MP4
            +-- Linux: X11 or portal screencast, OpenH264, MP4
```

The service lifecycle stays `Running` while a recording is active. `start_recording` and `stop_recording` are the only triggers. The shipped product configuration leaves the service disabled.

## Capture

The provider records the primary display at 10 frames per second. Width and height come from that display. H.264 needs even dimensions, so a provider crops an odd edge rather than scaling.

The capture thread and the encoder thread are separate from the supervisor thread. The frame queue holds two frames. A frame that arrives while the queue is full is dropped. Windows Graphics Capture and the Linux screencast stream deliver a frame when the display changes. The capture thread keeps the latest frame and submits it at the configured rate, so a still display still produces the requested duration. That retained frame is the only raw frame held after encoding.

`stop_recording` stops capture, flushes the encoder, finalizes the container, and only then reports the file. An empty or duration-less file is deleted and returned as a failure. A completed file stays on disk. Its path, size, and SHA-256 are stored in the local database. The video bytes are not. See [local-storage.md](local-storage.md). The service stays running so another recording can be requested. Shipped configuration leaves video disabled.

## Permission and health

Video requires `video_capture`. Health uses the same mapping as screenshot: available is healthy, a missing permission is `PermissionRequired`, an unavailable device is degraded, and a capture failure is unhealthy. Windows does not invent a consent grant. macOS uses the same Screen Recording preflight as screenshots and does not prompt. Linux does not open a portal during a health check.

## Files

Names are UTC `YYYYMMDD-HHMMSS-mmm.mp4`, with a numeric suffix when the name exists. The directory is `<data_dir>/videos`. Logs include duration, frame count, dropped frames, and byte size. They do not include the path or frame contents.
