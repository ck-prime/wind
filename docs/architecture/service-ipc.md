# Service IPC

The supervisor and an out-of-process service share a line protocol defined in `wind-core`. Protocol version is `1`. A peer that sends another version is rejected:

```text
Incompatible service protocol
expected: 1
received: 2
```

The supervisor accepts `hello`, `health`, `current_application`, and `shutdown`. The service answers with its service id, version, protocol version, capabilities, health, and errors. The service id is the stable id, such as `app_tracker`. The version is the implementation version. They are different fields.

## Transport

The supervisor starts the executable with `Command`. It does not use a shell. The protocol travels on the child's stdin and stdout. Those pipes exist only for that parent and child. There is no listening port, named endpoint, or network address, so another process cannot connect to the service. Stderr remains the log stream and is not parsed as protocol.

This is the local IPC for Windows, macOS, and Linux in this phase. A later transport can replace the pipes without changing the messages. macOS and Linux launches are not runtime tested here.

The launch clears `WIND_SERVICE_TEST_MODE`. No authentication token, password, or secret store material is placed on the command line or in the protocol.

## Identity check

Before start, the path must be absolute and the file name must be the platform executable name (`wind-app-tracker.exe` on Windows). After start, the handshake must report the expected service id, the manifest version, and protocol `1`. A mismatch stops that process. The supervisor does not treat the handshake as a granted permission. Permission checks still go through `PermissionProvider`.

## Application tracker

`wind-app-tracker` answers foreground queries. It does not open `wind.db`. The supervisor keeps the shared store and writes application segments after the query returns. Screenshot, activity, URL, and video services still run in the supervisor process.

## Shutdown

The supervisor writes `shutdown`, waits up to two seconds, and terminates the process only if it is still running. Dropping the handle also terminates a process that is still alive, so a failed start does not leave an orphan.
