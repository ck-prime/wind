//! Browser identity from an executable path.
//!
//! Classification uses the file name only. Window titles, page titles, and
//! URL text are not browser identity.

use std::path::Path;

use crate::contract::BrowserKind;

/// Map a platform executable path to a supported browser.
pub fn classify_browser_executable(executable: &str) -> Option<BrowserKind> {
    let name = Path::new(executable)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(executable);
    match name.to_ascii_lowercase().as_str() {
        "chrome.exe" | "google chrome" | "chrome" => Some(BrowserKind::Chrome),
        "chromium.exe" | "chromium" | "chrome-bin.exe" => Some(BrowserKind::Chromium),
        "msedge.exe" | "microsoft edge" | "msedge" => Some(BrowserKind::Edge),
        "firefox.exe" | "firefox" => Some(BrowserKind::Firefox),
        "safari.exe" | "safari" => Some(BrowserKind::Safari),
        _ => None,
    }
}

/// Bundle identifiers used by the macOS provider.
pub fn classify_browser_bundle(bundle_id: &str) -> Option<BrowserKind> {
    match bundle_id {
        "com.google.Chrome" => Some(BrowserKind::Chrome),
        "org.chromium.Chromium" => Some(BrowserKind::Chromium),
        "com.microsoft.edgemac" => Some(BrowserKind::Edge),
        "org.mozilla.firefox" => Some(BrowserKind::Firefox),
        "com.apple.Safari" => Some(BrowserKind::Safari),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_browser_bundle, classify_browser_executable};
    use crate::BrowserKind;

    #[test]
    fn known_executables_map_to_browsers() {
        assert_eq!(
            classify_browser_executable(r"C:\Program Files\Google\Chrome\Application\chrome.exe"),
            Some(BrowserKind::Chrome)
        );
        assert_eq!(
            classify_browser_executable(
                r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
            ),
            Some(BrowserKind::Edge)
        );
        assert_eq!(
            classify_browser_executable(r"C:\Program Files\Mozilla Firefox\firefox.exe"),
            Some(BrowserKind::Firefox)
        );
        assert_eq!(
            classify_browser_executable("/usr/bin/chromium"),
            Some(BrowserKind::Chromium)
        );
        assert_eq!(
            classify_browser_executable("/Applications/Safari.app/Contents/MacOS/Safari"),
            Some(BrowserKind::Safari)
        );
    }

    #[test]
    fn other_applications_are_not_browsers() {
        assert_eq!(
            classify_browser_executable(r"C:\Windows\explorer.exe"),
            None
        );
        assert_eq!(classify_browser_executable("code.exe"), None);
        assert_eq!(classify_browser_bundle("com.apple.Terminal"), None);
        assert_eq!(
            classify_browser_bundle("com.google.Chrome"),
            Some(BrowserKind::Chrome)
        );
    }
}
