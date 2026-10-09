# macOS

Future home for macOS API bindings. Nothing here is implemented.

Expected later residents include ScreenCaptureKit, NSWorkspace, Input Monitoring, TCC, and LaunchAgent installation. Business logic must call a platform abstraction rather than these APIs directly.

`wind-macos` checks Screen Recording with `CGPreflightScreenCaptureAccess` and captures one full display with ScreenCaptureKit. Video uses a ScreenCaptureKit stream and `SCRecordingOutput` to write an H.264 MP4. That recording API requires macOS 15. The crate does not prompt, read TCC, or install a LaunchAgent. A macOS build needs Rust 1.85 or newer and the Swift toolchain used by `screencapturekit`. The package is excluded from workspace members so a Windows build does not need Swift.

```bash
cargo test --manifest-path platforms/macos/Cargo.toml
```
