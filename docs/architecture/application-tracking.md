# Application tracking

The application tracker answers one question: which application is in the foreground. It does not infer a URL, read a window title, or inspect process memory.

```text
App tracker service
    |
    +-- current_application()
    |
    +-- AppTrackerProvider
            |
            +-- Windows: GetForegroundWindow, limited process query
            +-- macOS: NSWorkspace frontmost application
            +-- Linux: X11 _NET_ACTIVE_WINDOW and /proc/<pid>/exe
```

There is no background poll. A caller asks for the current application. The same application id on the next query is not another change. The first observation, and a later different id, are changes. Timestamps are `SystemTime` instants.

## Identity

| Platform | Stable id | Name |
| --- | --- | --- |
| Windows | Normalized executable path, lowercased, without a `\\?\` prefix | Executable file stem |
| macOS | Bundle identifier | Localized application name |
| Linux X11 | Executable path from `/proc/<pid>/exe` | Executable file name |
| Linux Wayland | Unsupported | The compositor does not expose a supported foreground query |
| Linux headless | Unavailable | No `DISPLAY` and no `WAYLAND_DISPLAY` |

A process id is not the application id. Window titles, command lines, and environment variables are not collected. Windows opens the process with `PROCESS_QUERY_LIMITED_INFORMATION` and closes the handle before returning. An access denial is unavailable, not a crash of the service.

When both `DISPLAY` and `WAYLAND_DISPLAY` are set, Linux uses the X11 path. Wayland-native clients that do not appear there are not invented.

## Health

Available means the query API can be used. A missing interactive session follows the existing session rule and becomes unavailable. An unsupported session is `Unsupported`. A failed query is unhealthy, and the service stays `Running` so another query can be made. `monitoring_enabled` stays false.

Logs use `application_changed name=...` or a bundle id that is not a filesystem path. They do not include the executable path.
