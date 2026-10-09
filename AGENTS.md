# Agent guide

Wind is a cross-platform desktop agent. The workspace, configuration, supervisor, and service runtime are in place. Screenshot capture, explicit video recording, foreground application tracking, browser URL observations, aggregate activity counts, and local durable storage are implemented. Upload is not.

Complete the requested phase. Do not start the next one in the same change.

## Product identity

- Do not hard-code the product display name in Rust.
- `config/product.toml` is the only branding source.
- Only the `wind-config` crate may parse `product.toml`.
- Do not copy product fields into other files to "make them available."
- Keep display branding separate from technical identity.
- `product.identifier` stays stable when `display_name` changes.
- The executable name `wind` is a technical command name, not the display name.

## Services

- Service IDs are stable: `screenshot`, `activity`, `app_tracker`, `url_tracker`, `video`.
- Define those IDs in `wind-core`. Do not scatter string literals.
- Each service crate owns its package version. Do not force services to share one workspace version.
- The supervisor registers services and is the composition root.
- The service manager owns lifecycle transitions. Services do not change that state themselves.
- Capability crates depend on `wind-core`, not on OS-specific code.
- Do not spawn processes or run a shell from the service manager.

## Platform code

- Do not put `#[cfg(target_os = ...)]` in business logic.
- Detect the host, session, and runtime paths through `wind-platform`.
- OS API bindings belong under `platforms/macos`, `platforms/windows`, and `platforms/linux`.
- Screenshot capture belongs in `wind-screenshot` and the platform provider. Video recording belongs in `wind-video` and the platform provider. Foreground application identity belongs in `wind-app-tracker`. Browser URL observations belong in `wind-url-tracker`. Aggregate activity counts belong in `wind-activity`. Raw key codes, characters, and pointer coordinates stay inside the platform listener and are not stored.
- Do not install a LaunchAgent, Windows service, or systemd unit from the runtime.
- Do not grant, bypass, or automatically request an operating-system permission.

## Dependencies and files

- Do not add a dependency unless a crate needs it.
- Do not create files that the current phase does not use.
- Do not modify unrelated files.
- Do not restructure the workspace without a concrete dependency or ownership problem.

## Runtime boundary

Service lifecycle, health, enablement, and service shutdown belong in the service manager. The supervisor owns process lifetime, the shutdown coordinator, runtime state, and platform paths. Do not implement these until a later phase explicitly asks for them:

- Storing raw keyboard or mouse input. Aggregate activity counts and local storage of finalized records are implemented. Screenshot capture, video recording, foreground application queries, and browser URL observations stay explicit and must not be extended into a scheduler or an uploader in an unrelated change
- Permission prompts or privacy-database writes
- Authentication, tokens, or account state
- Backend clients, upload, or remote logging
- IPC transport
- Installers, system services, or launch agents
- Updaters, signature checks, or rollback

## Security

1. Use least privilege.
2. Services must only receive permissions required by their feature.
3. The supervisor must not automatically receive every capability permission.
4. Never hard-code secrets.
5. Never hard-code authentication tokens.
6. Keep display branding separate from technical identity.
7. Keep service IDs stable.
8. Production releases must eventually be signed.
9. Future updates must verify artifact authenticity before installation.
10. Never bypass OS privacy or security controls.
11. Do not introduce root or admin privileges without a concrete requirement.
12. Do not implement stealth or anti-user mechanisms.
13. Do not collect monitoring data until the corresponding feature and permission model has been explicitly designed.

## Errors and logging

- Return errors that retain component, operation, service, platform, and cause when those facts exist.
- Do not use `unwrap`, `expect`, or `panic` on production paths.
- Log locally. Do not add telemetry.

## Tests

- Cover configuration loading, duplicate service rejection, service versions, and platform detection when changing those areas.
- Keep tests next to the crate they cover.
- Do not build a separate test harness for a small unit test.
