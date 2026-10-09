# Product configuration

`product.toml` is the only file that defines product branding and the technical product identifier.

The `wind-config` crate loads and validates this file. Other crates must not parse it.

| Field | Role |
| --- | --- |
| `product.name` | Short product name for future package metadata |
| `product.display_name` | Visible name used by the supervisor report and future UI |
| `product.identifier` | Stable technical identity (`com.wind.agent`) |
| `product.version` | Product version, independent of each service crate version |
| `branding.logo` | Workspace-relative path to the logo |
| `company.name` | Company name for future installer and about surfaces |
| `services.<id>.enabled` | Whether the supervisor may start that service |
| `runtime.health_check_interval_seconds` | How often the resident process reads service health. Default 10. Must be greater than zero |
| `runtime.shutdown_timeout_seconds` | Budget for one service stop. Default 5. Must be greater than zero |

Service identifiers are not stored here. They are stable code constants combined with `product.identifier`:

```text
{product.identifier}.supervisor
{product.identifier}.screenshot
{product.identifier}.activity
{product.identifier}.app-tracker
{product.identifier}.url-tracker
{product.identifier}.video
```

Changing `display_name` does not change those identifiers, the executable name, or service IDs.

Service tables use the stable IDs `screenshot`, `activity`, `app_tracker`, `url_tracker`, and `video`. If a table is omitted, that service is enabled. If the table is present, `enabled` is required. An unknown service name is rejected. `enabled = false` registers the service as `Disabled` and does not start it. That state is not a startup failure.

Lookup order for development builds:

1. `WIND_PRODUCT_CONFIG`, when it points at a file
2. `config/product.toml` walking up from the current directory
3. The same relative path walking up from the executable
4. The same relative path walking up from the config crate manifest

Release builds embed `config/product.toml` and `assets/branding/logo.svg`. They do not read those files from the working directory, the executable directory, or the source tree.

Logo paths in a development build are relative to the parent of the `config` directory. They must stay inside that tree (`..` is rejected). The release executable uses the embedded logo bytes for that same relative path.
