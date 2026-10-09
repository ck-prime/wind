# Service versioning

The product identity stays `com.wind.agent`. It comes from `config/product.toml` and does not include a service version, an executable name, or an executable path. Service technical ids stay:

```text
com.wind.agent.screenshot
com.wind.agent.activity
com.wind.agent.app-tracker
com.wind.agent.url-tracker
com.wind.agent.video
```

Service ids stay `screenshot`, `activity`, `app_tracker`, `url_tracker`, and `video` across versions. `ServiceVersion` is the existing `major.minor.patch` `Version` type. Each service reports:

```text
service_id
service_version
protocol_version
capabilities
health
```

The supervisor records the installed version from the service manifest or from the in-process crate version. The executable file name is not the version source.

Different services may run different versions at the same time. They interoperate only when `protocol_version` is `1`. A manifest that declares another protocol is rejected and that service is not started.

## Permission continuity

Permission state belongs to the operating system and the stable product identity. Replacing `screenshot` 1.0.0 with 2.0.0 does not create `com.wind.agent.screenshot.2` and does not request the permission again by itself. After a version change the supervisor calls `PermissionProvider::check` again. `Granted` is used only when that check says so. `PermissionRequired`, `Denied`, `Unsupported`, and `Unavailable` are reported as they are. The service does not cache a grant and does not write an OS permission database.

## Layout

Versioned executables live apart from user data:

```text
services/<technical-component>/<version>/wind-<technical-component>
```

User data stays under the existing runtime directories: `data/wind.db`, screenshots, videos, config, logs, and the secret store. The manifest in the runtime directory, or `WIND_SERVICE_MANIFEST`, names the active executable. It is metadata. It is not the permission authority and it is not the SQLite database.

## Process state

`ProcessState` is separate from lifecycle, health, capability status, and permission state:

```text
NotInstalled Installed Starting Running Stopping Stopped Crashed Updating Rollback
```
