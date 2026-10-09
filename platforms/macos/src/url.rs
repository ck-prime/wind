//! Current browser URL on macOS.
//!
//! The frontmost bundle identifier selects a supported browser. Accessibility
//! then reads the address field. The walk does not enter web areas, so page
//! text is not read. This module does not prompt, click, type, or send Apple
//! events. It is not runtime-tested from a Windows build.

#![allow(unsafe_code)]

use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr;
use std::time::SystemTime;

use wind_core::CapabilityStatus;
use wind_url_tracker::{
    classify_browser_bundle, sanitize_address_bar, BrowserIntegration, BrowserKind, UrlError,
    UrlObservation, UrlTrackerProvider,
};

const SUCCESS: i32 = 0;
const MAX_CHILDREN: i64 = 40;
const MAX_DEPTH: usize = 8;
const MAX_NODES: usize = 300;
const UTF8: u32 = 0x0800_0100;

/// On-demand macOS URL provider.
pub struct MacosUrlProvider {
    started: bool,
    status: CapabilityStatus,
    reason: Option<String>,
}

impl Default for MacosUrlProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MacosUrlProvider {
    pub fn new() -> Self {
        Self {
            started: false,
            status: CapabilityStatus::Unknown,
            reason: None,
        }
    }
}

impl UrlTrackerProvider for MacosUrlProvider {
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
        if self.status == CapabilityStatus::PermissionRequired {
            return Err(UrlError::Unavailable {
                reason: self
                    .reason
                    .clone()
                    .unwrap_or_else(|| "Accessibility permission is required".to_string()),
            });
        }
        if self.status != CapabilityStatus::Available {
            return Err(UrlError::Unavailable {
                reason: self
                    .reason
                    .clone()
                    .unwrap_or_else(|| "Accessibility is unavailable".to_string()),
            });
        }
        let application = crate::current_application().map_err(|error| UrlError::QueryFailed {
            reason: error.to_string(),
        })?;
        let Some(application) = application else {
            return Ok(None);
        };
        let Some(browser) = classify_browser_bundle(&application.application_id) else {
            return Ok(None);
        };
        let Some(raw) = read_address(browser)? else {
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
        if !trusted() {
            self.status = CapabilityStatus::PermissionRequired;
            self.reason = Some("Accessibility permission is required".to_string());
            return Ok(());
        }
        self.status = CapabilityStatus::Available;
        self.reason = None;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), UrlError> {
        self.started = false;
        Ok(())
    }
}

struct AddressSpec {
    identifiers: &'static [&'static str],
    descriptions: &'static [&'static str],
}

fn address_spec(browser: BrowserKind) -> AddressSpec {
    match browser {
        BrowserKind::Chrome | BrowserKind::Chromium | BrowserKind::Edge => AddressSpec {
            identifiers: &["omnibox", "addressEditBox"],
            descriptions: &["Address and search bar"],
        },
        BrowserKind::Firefox => AddressSpec {
            identifiers: &["urlbar-input", "urlbar"],
            descriptions: &["Search with Google or enter address"],
        },
        BrowserKind::Safari => AddressSpec {
            identifiers: &[],
            descriptions: &["Address and Search"],
        },
    }
}

fn read_address(browser: BrowserKind) -> Result<Option<String>, UrlError> {
    let system = ax_element(unsafe { AXUIElementCreateSystemWide() })?;
    let application = copy_element(&system, "AXFocusedApplication")?;
    let Some(application) = application else {
        return Ok(None);
    };
    let window = copy_element(&application, "AXFocusedWindow")?;
    let Some(window) = window else {
        return Ok(None);
    };
    find_address(&window, browser, 0, &mut 0)
}

fn find_address(
    element: &Ax,
    browser: BrowserKind,
    depth: usize,
    visited: &mut usize,
) -> Result<Option<String>, UrlError> {
    if depth > MAX_DEPTH || *visited >= MAX_NODES {
        return Ok(None);
    }
    *visited += 1;
    let role = string_attr(element, "AXRole").unwrap_or_default();
    if role == "AXWebArea" {
        return Ok(None);
    }
    let spec = address_spec(browser);
    if (role == "AXTextField" || role == "AXComboBox") && identity_matches(element, &spec) {
        if let Some(value) = string_attr(element, "AXValue") {
            if !value.is_empty() {
                return Ok(Some(value));
            }
        }
    }
    let children = children_of(element);
    for child in children.iter().take(MAX_CHILDREN as usize) {
        if let Some(value) = find_address(child, browser, depth + 1, visited)? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn identity_matches(element: &Ax, spec: &AddressSpec) -> bool {
    let identifier = string_attr(element, "AXIdentifier").unwrap_or_default();
    if spec
        .identifiers
        .iter()
        .any(|candidate| identifier.eq_ignore_ascii_case(candidate))
    {
        return true;
    }
    let description = string_attr(element, "AXDescription").unwrap_or_default();
    spec.descriptions
        .iter()
        .any(|candidate| description.eq_ignore_ascii_case(candidate))
}

struct Ax(*mut c_void);

impl Drop for Ax {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}

fn ax_element(pointer: *mut c_void) -> Result<Ax, UrlError> {
    if pointer.is_null() {
        Err(UrlError::Unavailable {
            reason: "Accessibility returned no element".to_string(),
        })
    } else {
        Ok(Ax(pointer))
    }
}

fn copy_element(element: &Ax, attribute: &str) -> Result<Option<Ax>, UrlError> {
    let Some(value) = copy_attribute(element, attribute)? else {
        return Ok(None);
    };
    let pointer = value.0;
    std::mem::forget(value);
    Ok(Some(Ax(pointer as *mut c_void)))
}

fn copy_attribute(element: &Ax, attribute: &str) -> Result<Option<Cf>, UrlError> {
    let name = cf_string(attribute)?;
    let mut value = ptr::null();
    let status = unsafe { AXUIElementCopyAttributeValue(element.0, name.0, &mut value) };
    if status == SUCCESS && !value.is_null() {
        Ok(Some(Cf(value)))
    } else if status == SUCCESS {
        Ok(None)
    } else {
        Err(UrlError::Unavailable {
            reason: "Accessibility attribute is not readable".to_string(),
        })
    }
}

fn string_attr(element: &Ax, attribute: &str) -> Option<String> {
    let value = copy_attribute(element, attribute).ok()??;
    cf_to_string(value.0)
}

fn children_of(element: &Ax) -> Vec<Ax> {
    let Ok(Some(value)) = copy_attribute(element, "AXChildren") else {
        return Vec::new();
    };
    if unsafe { CFGetTypeID(value.0) } != unsafe { CFArrayGetTypeID() } {
        return Vec::new();
    }
    let count = unsafe { CFArrayGetCount(value.0) }.clamp(0, MAX_CHILDREN as isize);
    let mut children = Vec::with_capacity(count as usize);
    for index in 0..count {
        let child = unsafe { CFArrayGetValueAtIndex(value.0, index) };
        if !child.is_null() {
            unsafe { CFRetain(child) };
            children.push(Ax(child as *mut c_void));
        }
    }
    children
}

struct Cf(*const c_void);

impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}

fn cf_string(text: &str) -> Result<Cf, UrlError> {
    let c_text = CString::new(text).map_err(|_| UrlError::QueryFailed {
        reason: "accessibility attribute name is invalid".to_string(),
    })?;
    let created = unsafe { CFStringCreateWithCString(ptr::null(), c_text.as_ptr(), UTF8) };
    if created.is_null() {
        Err(UrlError::Unavailable {
            reason: "CoreFoundation string allocation failed".to_string(),
        })
    } else {
        Ok(Cf(created))
    }
}

fn cf_to_string(value: *const c_void) -> Option<String> {
    if value.is_null() || unsafe { CFGetTypeID(value) } != unsafe { CFStringGetTypeID() } {
        return None;
    }
    let mut buffer = vec![0u8; 4096];
    let copied = unsafe {
        CFStringGetCString(
            value,
            buffer.as_mut_ptr().cast::<c_char>(),
            buffer.len() as isize,
            UTF8,
        )
    };
    if !copied {
        return None;
    }
    let text = unsafe { CStr::from_ptr(buffer.as_ptr().cast::<c_char>()) };
    Some(text.to_string_lossy().into_owned())
}

fn trusted() -> bool {
    accessibility_granted()
}

/// Whether Accessibility trust is on. This does not prompt or change it.
pub fn accessibility_granted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCreateSystemWide() -> *mut c_void;
    fn AXUIElementCopyAttributeValue(
        element: *mut c_void,
        attribute: *const c_void,
        value: *mut *const c_void,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(value: *const c_void);
    fn CFRetain(value: *const c_void) -> *const c_void;
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        text: *const c_char,
        encoding: u32,
    ) -> *const c_void;
    fn CFStringGetCString(
        value: *const c_void,
        buffer: *mut c_char,
        size: isize,
        encoding: u32,
    ) -> bool;
    fn CFGetTypeID(value: *const c_void) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFArrayGetCount(value: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(value: *const c_void, index: isize) -> *const c_void;
}
