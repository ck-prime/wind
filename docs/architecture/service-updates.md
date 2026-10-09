# Service updates

`UpdateManager` is the local update transaction. It does not contact a network and it does not trust a file name, a URL, or a manifest version by itself.

```text
stage
    |
verify identity, protocol, platform, architecture, SHA-256, signature
    |
stop old service
    |
activate the new version directory
    |
start and health-check
    |
keep the previous version
```

If the health check fails:

```text
stop the new service
    |
restore the previous version
    |
the previous directory is still on disk
```

Both version directories remain. The active pointer is a file named `active` under the service component directory. `previous` records the version that was active before the switch. Rollback reads `previous` and makes it active again. The first install has no previous version, so rollback fails cleanly.

## Verification

`PackageVerifier` checks the package bytes. `ClosedVerifier` rejects every package. That is what production code uses until a publisher key exists. Tests use `FixtureVerifier`, which accepts a package only when the SHA-256 matches and the signature bytes equal that digest. The fixture is not a private key and it is not a production trust root. No signing key is stored in the repository.

A package for the wrong service, platform, architecture, or protocol version is not staged. Protocol `2` produces:

```text
Incompatible service protocol expected: 1 received: 2
```

Only the services whose packages were staged are activated. A screenshot update does not replace the activity executable.

## What is not in this phase

No download, update server, installer, or automatic check. The supervisor does not fetch executables. A later updater can call `stage`, `commit`, and `rollback` with a signed manifest. User data, `wind.db`, and the secret store are outside the version directories and are not part of the transaction.
