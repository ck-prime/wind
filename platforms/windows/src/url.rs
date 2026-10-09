//! Current browser URL on Windows.
//!
//! The foreground executable selects a supported browser. UI Automation then
//! reads the address field's value pattern. The walk does not enter document
//! controls, so page text is not read. The window title is not a URL. This
//! module does not type, click, inject, or read browser storage.

use std::cell::Cell;
use std::time::SystemTime;

use wind_core::{emit, CapabilityStatus, Level, LogFields};
use wind_url_tracker::{
    classify_browser_executable, sanitize_address_bar, BrowserIntegration, BrowserKind, UrlError,
    UrlObservation, UrlTrackerProvider,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationElementArray,
    IUIAutomationValuePattern, TreeScope_Children, UIA_ComboBoxControlTypeId,
    UIA_DocumentControlTypeId, UIA_EditControlTypeId, UIA_ValuePatternId, UIA_CONTROLTYPE_ID,
};

const MAX_NODES: usize = 300;
const MAX_CHILDREN: i32 = 40;
const MAX_DEPTH: usize = 8;

/// On-demand Windows URL provider. It does not listen in the background.
pub struct WindowsUrlProvider {
    started: bool,
    unavailable: Option<String>,
}

impl Default for WindowsUrlProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsUrlProvider {
    pub fn new() -> Self {
        Self {
            started: false,
            unavailable: None,
        }
    }
}

impl UrlTrackerProvider for WindowsUrlProvider {
    fn capability_status(&self) -> CapabilityStatus {
        if self.unavailable.is_some() || !self.started {
            CapabilityStatus::Unavailable
        } else {
            CapabilityStatus::Available
        }
    }

    fn integration(&self) -> BrowserIntegration {
        if self.unavailable.is_some() || !self.started {
            BrowserIntegration::Unavailable
        } else {
            BrowserIntegration::Connected
        }
    }

    fn current_url(&self) -> Result<Option<UrlObservation>, UrlError> {
        if !self.started {
            return Err(UrlError::Stopped);
        }
        if let Some(reason) = &self.unavailable {
            return Err(UrlError::Unavailable {
                reason: reason.clone(),
            });
        }
        let foreground =
            crate::app::foreground_process().map_err(|error| UrlError::QueryFailed {
                reason: error.to_string(),
            })?;
        let Some(foreground) = foreground else {
            return Ok(None);
        };
        let Some(executable) = foreground.application.executable.as_deref() else {
            return Ok(None);
        };
        let Some(browser) = classify_browser_executable(executable) else {
            log_provider("url_provider_unsupported");
            return Ok(None);
        };
        log_provider(&format!(
            "url_provider_browser_detected browser={}",
            browser.as_str()
        ));
        ensure_com()?;
        let automation = automation()?;
        let root = unsafe { automation.ElementFromHandle(foreground.window) }.map_err(|error| {
            UrlError::Unavailable {
                reason: error.to_string(),
            }
        })?;
        let mut skipped = false;
        let Some(raw) = find_address_value(&automation, &root, browser, 0, &mut 0, &mut skipped)?
        else {
            log_provider("url_provider_address_field_unavailable");
            if skipped {
                return Ok(None);
            }
            return Err(UrlError::Unavailable {
                reason: "browser address field is not exposed".to_string(),
            });
        };
        let url = sanitize_address_bar(&raw).map_err(|error| UrlError::QueryFailed {
            reason: error.to_string(),
        })?;
        Ok(Some(UrlObservation {
            browser,
            url,
            observed_at: SystemTime::now(),
        }))
    }

    fn start(&mut self) -> Result<(), UrlError> {
        ensure_com()?;
        match automation() {
            Ok(_) => {
                self.started = true;
                self.unavailable = None;
                Ok(())
            }
            Err(error) => {
                self.started = true;
                self.unavailable = Some(error.to_string());
                Ok(())
            }
        }
    }

    fn stop(&mut self) -> Result<(), UrlError> {
        self.started = false;
        Ok(())
    }
}

struct AddressSpec {
    automation_ids: &'static [&'static str],
    names: &'static [&'static str],
}

fn address_spec(browser: BrowserKind) -> AddressSpec {
    match browser {
        BrowserKind::Chrome | BrowserKind::Chromium | BrowserKind::Edge => AddressSpec {
            automation_ids: &["omnibox", "addressEditBox", "addressBar"],
            names: &["Address and search bar"],
        },
        BrowserKind::Firefox => AddressSpec {
            automation_ids: &["urlbar-input", "urlbar"],
            names: &["Search with Google or enter address"],
        },
        BrowserKind::Safari => AddressSpec {
            automation_ids: &[],
            names: &[],
        },
    }
}

fn find_address_value(
    automation: &IUIAutomation,
    element: &IUIAutomationElement,
    browser: BrowserKind,
    depth: usize,
    visited: &mut usize,
    skipped: &mut bool,
) -> Result<Option<String>, UrlError> {
    if depth > MAX_DEPTH || *visited >= MAX_NODES {
        return Ok(None);
    }
    *visited += 1;
    let spec = address_spec(browser);
    let control = control_type(element);
    if control
        .is_some_and(|control| is_address_control(control) && identity_matches(element, &spec))
    {
        if let Some(value) = value_pattern(element) {
            if sanitize_address_bar(&value).is_ok() {
                return Ok(Some(value));
            }
            if !value.is_empty() {
                *skipped = true;
            }
        }
    }
    if control == Some(UIA_DocumentControlTypeId) {
        return Ok(None);
    }
    let children = match children_of(automation, element) {
        Ok(children) => children,
        Err(_) => return Ok(None),
    };
    let count = unsafe { children.Length() }.unwrap_or(0).min(MAX_CHILDREN);
    for index in 0..count {
        let Ok(child) = (unsafe { children.GetElement(index) }) else {
            continue;
        };
        if let Some(value) =
            find_address_value(automation, &child, browser, depth + 1, visited, skipped)?
        {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn is_address_control(control: UIA_CONTROLTYPE_ID) -> bool {
    control == UIA_EditControlTypeId || control == UIA_ComboBoxControlTypeId
}

fn identity_matches(element: &IUIAutomationElement, spec: &AddressSpec) -> bool {
    let automation_id = bstr_text(unsafe { element.CurrentAutomationId() }.ok());
    if spec
        .automation_ids
        .iter()
        .any(|candidate| automation_id.eq_ignore_ascii_case(candidate))
    {
        return true;
    }
    let name = bstr_text(unsafe { element.CurrentName() }.ok());
    spec.names
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

fn value_pattern(element: &IUIAutomationElement) -> Option<String> {
    let pattern =
        unsafe { element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) }
            .ok()?;
    let value = unsafe { pattern.CurrentValue() }.ok()?;
    let text = value.to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn children_of(
    automation: &IUIAutomation,
    element: &IUIAutomationElement,
) -> Result<IUIAutomationElementArray, UrlError> {
    let condition =
        unsafe { automation.CreateTrueCondition() }.map_err(|error| UrlError::Unavailable {
            reason: error.to_string(),
        })?;
    unsafe { element.FindAll(TreeScope_Children, &condition) }.map_err(|error| {
        UrlError::Unavailable {
            reason: error.to_string(),
        }
    })
}

fn control_type(element: &IUIAutomationElement) -> Option<UIA_CONTROLTYPE_ID> {
    unsafe { element.CurrentControlType() }.ok()
}

fn bstr_text(value: Option<windows::core::BSTR>) -> String {
    value.map(|text| text.to_string()).unwrap_or_default()
}

fn automation() -> Result<IUIAutomation, UrlError> {
    unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }.map_err(|error| {
        UrlError::Unavailable {
            reason: error.to_string(),
        }
    })
}

fn ensure_com() -> Result<(), UrlError> {
    thread_local! {
        static READY: Cell<bool> = const { Cell::new(false) };
    }
    if READY.get() {
        return Ok(());
    }
    unsafe {
        let first = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if first.is_ok() || changed_mode(first) {
            READY.set(true);
            return Ok(());
        }
        let second = CoInitializeEx(None, COINIT_MULTITHREADED);
        if second.is_ok() || changed_mode(second) {
            READY.set(true);
            return Ok(());
        }
        Err(UrlError::Unavailable {
            reason: second.message().to_string(),
        })
    }
}

fn changed_mode(result: windows::core::HRESULT) -> bool {
    result.0 == 0x8001_0106_u32 as i32
}

fn log_provider(message: &str) {
    emit(&LogFields {
        level: Level::Info,
        component: "url_tracker",
        service_id: Some("url_tracker"),
        version: None,
        platform: Some("windows"),
        message,
        error: None,
        state: Some("Running"),
    });
}

#[cfg(test)]
mod tests {
    use super::WindowsUrlProvider;
    use wind_url_tracker::{UrlError, UrlTrackerProvider};

    #[test]
    fn a_query_returns_nothing_or_a_sanitized_http_url() {
        let mut provider = WindowsUrlProvider::new();
        provider.start().expect("start");
        match provider.current_url() {
            Ok(None) | Err(UrlError::Unavailable { .. }) => {}
            Ok(Some(observation)) => {
                assert!(
                    observation.url.starts_with("http://")
                        || observation.url.starts_with("https://")
                );
                assert!(!observation.url.contains('@'));
                assert!(!observation.url.contains('#'));
                let rendered = format!("{observation:?}");
                assert!(!rendered.contains("http"));
            }
            Err(error) => panic!("unexpected url query failure: {error}"),
        }
        provider.stop().expect("stop");
    }
}
