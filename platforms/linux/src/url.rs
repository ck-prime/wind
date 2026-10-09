//! Current browser URL on Linux.
//!
//! A supported browser is identified from its executable when X11 can report
//! the foreground process, and from the AT-SPI application name otherwise.
//! The address field is read from the AT-SPI tree. Document nodes are not
//! walked. Wayland is not bypassed: if AT-SPI is absent, the URL is
//! unavailable. This module is not runtime-tested from a Windows build.

use std::env;
use std::time::SystemTime;

use wind_core::CapabilityStatus;
use wind_url_tracker::{
    classify_browser_executable, sanitize_address_bar, BrowserIntegration, BrowserKind, UrlError,
    UrlObservation, UrlTrackerProvider,
};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};

const MAX_CHILDREN: i32 = 40;
const MAX_DEPTH: usize = 8;
const MAX_NODES: usize = 300;
const ROLE_ENTRY: u32 = 79;
const ROLE_COMBO_BOX: u32 = 50;
const ROLE_DOCUMENT_WEB: u32 = 94;
const STATE_ACTIVE: u32 = 1;

/// On-demand Linux URL provider.
pub struct LinuxUrlProvider {
    started: bool,
    status: CapabilityStatus,
    reason: Option<String>,
}

impl Default for LinuxUrlProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxUrlProvider {
    pub fn new() -> Self {
        Self {
            started: false,
            status: CapabilityStatus::Unknown,
            reason: None,
        }
    }
}

impl UrlTrackerProvider for LinuxUrlProvider {
    fn capability_status(&self) -> CapabilityStatus {
        if !self.started {
            CapabilityStatus::Unavailable
        } else {
            self.status
        }
    }

    fn integration(&self) -> BrowserIntegration {
        match self.capability_status() {
            CapabilityStatus::Available => BrowserIntegration::Connected,
            CapabilityStatus::Unsupported => BrowserIntegration::Unsupported,
            _ => BrowserIntegration::Unavailable,
        }
    }

    fn current_url(&self) -> Result<Option<UrlObservation>, UrlError> {
        if !self.started {
            return Err(UrlError::Stopped);
        }
        if self.status != CapabilityStatus::Available {
            return Err(UrlError::Unavailable {
                reason: self
                    .reason
                    .clone()
                    .unwrap_or_else(|| "AT-SPI is unavailable".to_string()),
            });
        }
        let connection = a11y_connection()?;
        let Some((browser, application)) = focused_browser(&connection)? else {
            return Ok(None);
        };
        let Some(raw) = find_address(&connection, &application, browser, 0, &mut 0)? else {
            return Err(UrlError::Unavailable {
                reason: "browser address field is not exposed".to_string(),
            });
        };
        match sanitize_address_bar(&raw) {
            Ok(url) => Ok(Some(UrlObservation {
                browser,
                url,
                observed_at: SystemTime::now(),
            })),
            Err(UrlError::InvalidUrl { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn start(&mut self) -> Result<(), UrlError> {
        self.started = true;
        match a11y_connection() {
            Ok(_) => {
                self.status = CapabilityStatus::Available;
                self.reason = None;
            }
            Err(error) => {
                self.status = CapabilityStatus::Unavailable;
                self.reason = Some(error.to_string());
            }
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<(), UrlError> {
        self.started = false;
        Ok(())
    }
}

struct Node {
    destination: String,
    path: OwnedObjectPath,
}

struct AddressSpec {
    names: &'static [&'static str],
}

fn address_spec(browser: BrowserKind) -> AddressSpec {
    match browser {
        BrowserKind::Chrome | BrowserKind::Chromium | BrowserKind::Edge => AddressSpec {
            names: &["Address and search bar"],
        },
        BrowserKind::Firefox => AddressSpec {
            names: &["Search with Google or enter address", "Navigation"],
        },
        BrowserKind::Safari => AddressSpec { names: &[] },
    }
}

fn focused_browser(connection: &Connection) -> Result<Option<(BrowserKind, Node)>, UrlError> {
    let executable = foreground_executable();
    let desktop = desktop(connection)?;
    let count = child_count(connection, &desktop)?.min(MAX_CHILDREN);
    for index in 0..count {
        let Some(application) = child(connection, &desktop, index)? else {
            continue;
        };
        if !application_active(connection, &application) {
            continue;
        }
        if let Some(browser) = browser_of(connection, &application, executable.as_deref()) {
            return Ok(Some((browser, application)));
        }
        return Ok(None);
    }
    Ok(None)
}

fn browser_of(
    connection: &Connection,
    application: &Node,
    executable: Option<&str>,
) -> Option<BrowserKind> {
    if let Some(executable) = executable {
        if let Some(browser) = classify_browser_executable(executable) {
            return Some(browser);
        }
    }
    if let Some(pid) = application_pid(connection, application) {
        if let Some(path) = std::fs::read_link(format!("/proc/{pid}/exe"))
            .ok()
            .and_then(|path| path.to_str().map(str::to_string))
        {
            if let Some(browser) = classify_browser_executable(&path) {
                return Some(browser);
            }
        }
    }
    let name = property_string(connection, application, "org.a11y.atspi.Accessible", "Name")
        .unwrap_or_default();
    match name.as_str() {
        "Google Chrome" | "Chrome" => Some(BrowserKind::Chrome),
        "Chromium" => Some(BrowserKind::Chromium),
        "Microsoft Edge" => Some(BrowserKind::Edge),
        "Firefox" => Some(BrowserKind::Firefox),
        _ => None,
    }
}

fn foreground_executable() -> Option<String> {
    crate::current_application()
        .ok()
        .flatten()
        .and_then(|application| application.executable)
}

fn find_address(
    connection: &Connection,
    node: &Node,
    browser: BrowserKind,
    depth: usize,
    visited: &mut usize,
) -> Result<Option<String>, UrlError> {
    if depth > MAX_DEPTH || *visited >= MAX_NODES {
        return Ok(None);
    }
    *visited += 1;
    let role = role_of(connection, node).unwrap_or(0);
    if role == ROLE_DOCUMENT_WEB {
        return Ok(None);
    }
    if (role == ROLE_ENTRY || role == ROLE_COMBO_BOX) && identity_matches(connection, node, browser)
    {
        if let Some(value) = text_value(connection, node) {
            if !value.is_empty() {
                return Ok(Some(value));
            }
        }
    }
    let count = child_count(connection, node).unwrap_or(0).min(MAX_CHILDREN);
    for index in 0..count {
        let Some(child) = child(connection, node, index)? else {
            continue;
        };
        if let Some(value) = find_address(connection, &child, browser, depth + 1, visited)? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn identity_matches(connection: &Connection, node: &Node, browser: BrowserKind) -> bool {
    let name =
        property_string(connection, node, "org.a11y.atspi.Accessible", "Name").unwrap_or_default();
    let description = property_string(connection, node, "org.a11y.atspi.Accessible", "Description")
        .unwrap_or_default();
    address_spec(browser).names.iter().any(|candidate| {
        name.eq_ignore_ascii_case(candidate) || description.eq_ignore_ascii_case(candidate)
    })
}

fn a11y_connection() -> Result<Connection, UrlError> {
    if env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() && env::var_os("XDG_RUNTIME_DIR").is_none()
    {
        return Err(UrlError::Unavailable {
            reason: "no session bus is available for AT-SPI".to_string(),
        });
    }
    let session = Connection::session().map_err(|error| UrlError::Unavailable {
        reason: error.to_string(),
    })?;
    let bus = proxy(&session, "org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus")?;
    let address: String = bus
        .call("GetAddress", &())
        .map_err(|error| UrlError::Unavailable {
            reason: error.to_string(),
        })?;
    zbus::blocking::connection::Builder::address(address.as_str())
        .map_err(|error| UrlError::Unavailable {
            reason: error.to_string(),
        })?
        .build()
        .map_err(|error| UrlError::Unavailable {
            reason: error.to_string(),
        })
}

fn desktop(connection: &Connection) -> Result<Node, UrlError> {
    let registry = proxy(
        connection,
        "org.a11y.atspi.Registry",
        "/org/a11y/atspi/registry",
        "org.a11y.atspi.Registry",
    )?;
    let (destination, path): (String, OwnedObjectPath) = registry
        .call("GetDesktop", &(0i32))
        .map_err(|error| UrlError::Unavailable {
            reason: error.to_string(),
        })?;
    Ok(Node { destination, path })
}

fn child(connection: &Connection, node: &Node, index: i32) -> Result<Option<Node>, UrlError> {
    let accessible = node_proxy(connection, node, "org.a11y.atspi.Accessible")?;
    let (destination, path): (String, OwnedObjectPath) = accessible
        .call("GetChildAtIndex", &(index))
        .map_err(|_| UrlError::Unavailable {
            reason: "AT-SPI child is not readable".to_string(),
        })?;
    if path.as_str() == "/org/a11y/atspi/null" || destination.is_empty() {
        return Ok(None);
    }
    Ok(Some(Node { destination, path }))
}

fn child_count(connection: &Connection, node: &Node) -> Result<i32, UrlError> {
    property_i32(connection, node, "org.a11y.atspi.Accessible", "ChildCount")
}

fn role_of(connection: &Connection, node: &Node) -> Result<u32, UrlError> {
    let accessible = node_proxy(connection, node, "org.a11y.atspi.Accessible")?;
    accessible
        .call("GetRole", &())
        .map_err(|_| UrlError::Unavailable {
            reason: "AT-SPI role is not readable".to_string(),
        })
}

fn text_value(connection: &Connection, node: &Node) -> Option<String> {
    let text = node_proxy(connection, node, "org.a11y.atspi.Text").ok()?;
    text.call("GetText", &(0i32, -1i32)).ok()
}

fn application_active(connection: &Connection, node: &Node) -> bool {
    let Ok(accessible) = node_proxy(connection, node, "org.a11y.atspi.Accessible") else {
        return false;
    };
    let Ok(states): Result<Vec<u32>, _> = accessible.call("GetState", &()) else {
        return false;
    };
    states.first().copied().unwrap_or(0) & (1 << STATE_ACTIVE) != 0
}

fn application_pid(connection: &Connection, node: &Node) -> Option<i32> {
    property_i32(connection, node, "org.a11y.atspi.Application", "Id").ok()
}

fn property_string(
    connection: &Connection,
    node: &Node,
    interface: &str,
    name: &str,
) -> Result<String, UrlError> {
    let value = property(connection, node, interface, name)?;
    String::try_from(value).map_err(|_| UrlError::Unavailable {
        reason: "AT-SPI property is not text".to_string(),
    })
}

fn property_i32(
    connection: &Connection,
    node: &Node,
    interface: &str,
    name: &str,
) -> Result<i32, UrlError> {
    let value = property(connection, node, interface, name)?;
    i32::try_from(value).map_err(|_| UrlError::Unavailable {
        reason: "AT-SPI property is not an integer".to_string(),
    })
}

fn property(
    connection: &Connection,
    node: &Node,
    interface: &str,
    name: &str,
) -> Result<OwnedValue, UrlError> {
    let properties = node_proxy(connection, node, "org.freedesktop.DBus.Properties")?;
    properties
        .call("Get", &(interface, name))
        .map_err(|_| UrlError::Unavailable {
            reason: "AT-SPI property is not readable".to_string(),
        })
}

fn node_proxy<'a>(
    connection: &'a Connection,
    node: &'a Node,
    interface: &'a str,
) -> Result<Proxy<'a>, UrlError> {
    proxy(connection, &node.destination, node.path.as_str(), interface)
}

fn proxy<'a>(
    connection: &'a Connection,
    destination: &'a str,
    path: &'a str,
    interface: &'a str,
) -> Result<Proxy<'a>, UrlError> {
    let path = ObjectPath::try_from(path).map_err(|_| UrlError::Unavailable {
        reason: "AT-SPI object path is invalid".to_string(),
    })?;
    Proxy::new(connection, destination, path, interface).map_err(|error| UrlError::Unavailable {
        reason: error.to_string(),
    })
}
