# Linux

`wind-linux` captures the primary display and records it. It is excluded from workspace members so Windows builds do not need X11 or PipeWire headers.

Screenshot and video use `xcap`.

- `DISPLAY` set and `WAYLAND_DISPLAY` unset: X11. Status can be `Available` when a monitor is enumerated.
- `WAYLAND_DISPLAY` set: status stays `PermissionRequired` until an explicit capture succeeds. That capture uses the compositor's screencast path and may show the system portal. A health check does not open the portal.
- Neither variable set: `Unavailable`. This is the headless case.
- A denial stays `PermissionRequired`. A missing capture library is `Unsupported` only when the error says the platform cannot do it.

Video encodes H.264 with OpenH264 and muxes MP4 with the `mp4` crate. The frame queue holds two frames and drops a new frame when it is full. The build downloads Cisco's OpenH264 binary. The crate is BSD-2-Clause and the binary is covered by Cisco's patent grant. Root is not required, and the provider does not inject into a compositor.

Build on Linux:

```bash
cargo test --manifest-path platforms/linux/Cargo.toml
```

System packages include pkg-config, libxcb, libxrandr, libpipewire, libwayland, and clang. Rust 1.85 or newer is required because `xcap` 0.9 uses the 2024 edition.
