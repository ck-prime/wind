# URL tracking

The URL tracker answers one question when asked: what `http` or `https` URL is in the foreground browser's address field. It does not install a browser component, register a browser host, or read window titles, browser process memory, cookies, page content, or a browser history database.

```text
Supervisor
    |
    +-- url_tracker service
            |
            +-- UrlTrackerProvider
                    |
                    +-- browser URL provider
                            |
                            +-- Windows UI Automation
                            +-- macOS Accessibility
                            +-- Linux AT-SPI
                                    |
                                    +-- existing sanitizer
                                    |
                                    +-- current observation
```

`current_url` reads the address field at the moment it is called. The service does not poll and does not keep a browser history. A URL segment is written to the local database when the sanitized URL changes or the service stops. No supported browser in front returns no URL. The service stays `Running`. The same browser and sanitized URL is not another change. See [local-storage.md](local-storage.md).

Nothing is written under `<data_dir>/urls`. Finalized URL rows live in the local database.

## Why the browser is left unchanged

Installing a component into the browser would require a host registration and permission to see tab URLs. This phase does not do that. The provider reads only the address field the browser already exposes in its accessibility tree. It does not type, click, focus that field, change how the browser is launched, or send keyboard or mouse input.

## Acquisition

The foreground application is identified from its executable, or on macOS from its bundle identifier. The window title, the page title, and the URL text are not used to decide which browser is in front.

| Browser | Identity |
| --- | --- |
| Chrome | `chrome.exe`, `Google Chrome`, `com.google.Chrome` |
| Edge | `msedge.exe`, `Microsoft Edge`, `com.microsoft.edgemac` |
| Firefox | `firefox.exe`, `Firefox`, `org.mozilla.firefox` |
| Chromium | `chromium` / `chromium.exe`, `org.chromium.Chromium` |
| Safari | `Safari`, `com.apple.Safari` |

Other applications return no URL.

The address field is located by control type, accessibility role, automation id, and a small set of accessible names. The walk skips document and web-area nodes, so page text is not read. The value pattern, `AXValue`, or AT-SPI text is then passed to the existing sanitizer. A field that is not an `http` or `https` URL returns no URL.

## Platforms

| Platform | Mechanism | Status |
| --- | --- | --- |
| Windows | UI Automation, normal user process | Implemented, compiled, and runtime-tested on Windows 11 x86_64 for Chrome 154.0.8037.57 and Edge 154.0.4258.53. Firefox is not installed on this machine, so it was not runtime-tested. |
| macOS | Accessibility (`AXUIElement`), read-only | Implemented and target-gated. Not compiled and not runtime-tested on this Windows machine. Missing Accessibility permission is `PermissionRequired`. Wind does not prompt or change TCC. |
| Linux | AT-SPI2 over the accessibility bus | Implemented and target-gated. Not compiled and not runtime-tested on this Windows machine. No session bus, or an address field that is not exposed, is `Unavailable`. Wayland is not bypassed and X11 is not required in order to cross a Wayland boundary. |

Windows UI Automation has no separate consent query. The permission answer stays `Unknown`. Wind does not invent a grant and does not elevate. If the browser window cannot be read, the provider reports `Unavailable` and the service becomes degraded. The supervisor keeps running.

## Sanitization

Only `http` and `https` URLs with a host are kept. A schemeless address-field value that contains a host, such as `example.com/path`, is read as `https` and then sanitized. Search text is left unread as a URL. Userinfo is removed, so `https://user:password@example.com/path?token=abc&a=1#secret` becomes `https://example.com/path?a=1`. The fragment is removed. Query names in the existing fixed list are removed, including `token`, `access_token`, `password`, `secret`, `api_key`, `code`, and `session`. This is not a complete secret detector.

Logs record browser identity, such as `url_observed browser=chrome`, and do not include the URL. The explicit `--current-url` command prints the sanitized URL for the operator who asked.

## Unsupported cases

- The foreground application is not a supported browser.
- The browser does not expose a stable address field.
- The address field contains text that is not an `http` or `https` URL.
- Windows cannot read the window because of an integrity-level boundary. Wind does not elevate.
- macOS Accessibility permission is not granted.
- Linux has no AT-SPI bus, or the bus does not expose the address field.
- Safari on Windows has no UI Automation address-field adapter.
