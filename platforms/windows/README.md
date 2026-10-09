# Windows

`wind-windows` captures the primary display, records it, and counts aggregate input with low-level hooks. The hooks do not store keys or pointer coordinates.

Screenshot uses Windows Graphics Capture through `xcap` (`wgc`) and encodes PNG with `png`. Video reads the same capture stream, keeps at most two frames queued, drops a new frame when the encoder is behind, and writes H.264 in an MP4 container with Media Foundation. A normal user account is enough. The crate does not inject code or install a Windows service.

Windows has no screen-recording consent query. The permission answer stays `Unknown`. The provider reports `Available` when a display can be enumerated. A runtime capture failure is classified from the error instead of being reported as `Unsupported`.

This crate is a workspace member, so `cargo test --workspace` builds and runs it on Windows.
