//! Open System Settings for a privacy pane, and stay with the person until
//! the switch is on.
//!
//! The dialog names the Privacy & Security list and stays up after Settings
//! opens. It does not read TCC, call a request API, or change a permission.

use std::process::{Command, Stdio};

use wind_core::{Capability, PermissionState};

/// Open `url` with the system `open` tool. No shell is used.
pub fn open_system_settings(url: &str) -> Result<(), ()> {
    match Command::new("open").arg(url).status() {
        Ok(status) if status.success() => Ok(()),
        _ => Err(()),
    }
}

/// Result of guiding one privacy switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionGuideOutcome {
    /// The preflight check now reports access.
    Granted,
    /// The person left this switch for later.
    Skipped,
    /// No dialog can be shown for this capability.
    Unavailable,
}

/// Open the matching Settings pane and keep a dialog up until the switch is on.
///
/// `stop` ends the guide between dialogs, for example when Wind is quitting.
/// The dialog does not grant the permission.
pub fn guide_permission_until_granted(
    display_name: &str,
    capability: Capability,
    step: usize,
    total: usize,
    stop: &dyn Fn() -> bool,
) -> PermissionGuideOutcome {
    if stop() {
        return PermissionGuideOutcome::Skipped;
    }
    if access_granted(capability) {
        return PermissionGuideOutcome::Granted;
    }
    if needs_relaunch(capability) {
        guide_relaunch(display_name, &[capability]);
        return PermissionGuideOutcome::Skipped;
    }
    let Some(pane) = pane_name(capability) else {
        return PermissionGuideOutcome::Unavailable;
    };
    let app = app_name(display_name);
    let _ = open_pane(capability);
    let mut waiting_on_settings = true;
    loop {
        if stop() {
            return PermissionGuideOutcome::Skipped;
        }
        let alert = if waiting_on_settings {
            turn_on_alert(&app, pane, step, total)
        } else {
            still_needed_alert(&app, pane)
        };
        match present_choice(&alert) {
            Ok(GuideChoice::Done) => {
                if access_granted(capability) {
                    return PermissionGuideOutcome::Granted;
                }
                waiting_on_settings = false;
            }
            Ok(GuideChoice::OpenSettings) => {
                let _ = open_pane(capability);
                waiting_on_settings = true;
            }
            Ok(GuideChoice::Skip) => return PermissionGuideOutcome::Skipped,
            Ok(GuideChoice::Reopen) => {
                crate::reopen_and_quit();
                return PermissionGuideOutcome::Skipped;
            }
            Err(()) => return PermissionGuideOutcome::Unavailable,
        }
    }
}

/// Guide each missing permission, then say what is still off.
pub fn guide_permissions_until_granted(
    display_name: &str,
    capabilities: &[Capability],
    stop: &dyn Fn() -> bool,
) {
    let pending: Vec<Capability> = capabilities
        .iter()
        .copied()
        .filter(|capability| pane_name(*capability).is_some() && !access_granted(*capability))
        .collect();
    if pending.is_empty() || stop() {
        return;
    }
    let relaunch: Vec<Capability> = pending
        .iter()
        .copied()
        .filter(|capability| needs_relaunch(*capability))
        .collect();
    if !relaunch.is_empty() {
        // macOS applies these only after Wind quits. Another dialog would cover
        // the system Quit & Reopen prompt, so the rest waits for the next launch.
        guide_relaunch(display_name, &relaunch);
        return;
    }
    let total = pending.len();
    let mut skipped = Vec::new();
    for (index, capability) in pending.into_iter().enumerate() {
        if stop() {
            return;
        }
        match guide_permission_until_granted(display_name, capability, index + 1, total, stop) {
            PermissionGuideOutcome::Granted => {}
            PermissionGuideOutcome::Skipped => {
                if let Some(pane) = pane_name(capability) {
                    skipped.push(pane);
                }
            }
            PermissionGuideOutcome::Unavailable => return,
        }
    }
    if stop() {
        return;
    }
    let _ = present_choice(&finished_alert(&app_name(display_name), &skipped));
}

struct PermissionAlert {
    title: String,
    message: String,
    buttons: &'static [&'static str],
}

fn guide_relaunch(display_name: &str, capabilities: &[Capability]) {
    let panes: Vec<&str> = capabilities
        .iter()
        .filter_map(|capability| pane_name(*capability))
        .collect();
    if panes.is_empty() {
        return;
    }
    if let Some(capability) = capabilities.first() {
        let _ = open_pane(*capability);
    }
    if matches!(
        present_choice(&relaunch_alert(&app_name(display_name), &panes)),
        Ok(GuideChoice::Reopen)
    ) {
        crate::reopen_and_quit();
    }
}

fn needs_relaunch(capability: Capability) -> bool {
    matches!(
        capability,
        Capability::ScreenCapture | Capability::VideoCapture | Capability::InputMonitoring
    )
}

fn relaunch_alert(app: &str, panes: &[&str]) -> PermissionAlert {
    let list = panes.join(" and ");
    let extra = if panes.len() > 1 {
        " Both lists are under Privacy & Security."
    } else {
        ""
    };
    PermissionAlert {
        title: format!("{app} needs to reopen"),
        message: format!(
            "Turn on {app} for {list}.{extra}\n\nIf {app} is not listed, click + and choose {app}.\n\nThen click Quit & Reopen. {app} closes and opens again by itself."
        ),
        buttons: &["Quit & Reopen"],
    }
}

fn turn_on_alert(app: &str, pane: &str, step: usize, total: usize) -> PermissionAlert {
    PermissionAlert {
        title: format!("{app}: {pane} ({step} of {total})"),
        message: format!(
            "Settings is open to {pane}.\n\nTurn on {app}. If it is not listed, click + and choose {app}.\n\nClick Done after the switch is on. {effect}",
            effect = pane_effect(app, pane)
        ),
        buttons: &["Skip", "Open Settings", "Done"],
    }
}

fn still_needed_alert(app: &str, pane: &str) -> PermissionAlert {
    PermissionAlert {
        title: format!("{pane} is still off"),
        message: format!(
            "Turn on {app} under Privacy & Security → {pane}, then click Done.\n\n{effect}",
            effect = pane_effect(app, pane)
        ),
        buttons: &["Skip", "Open Settings", "Done"],
    }
}

fn finished_alert(app: &str, skipped: &[&str]) -> PermissionAlert {
    if skipped.is_empty() {
        PermissionAlert {
            title: format!("{app} is ready"),
            message: format!("{app} can use those switches now and is already saving what it can."),
            buttons: &["OK"],
        }
    } else {
        PermissionAlert {
            title: format!("{app} still needs a switch"),
            message: format!(
                "Still off: {}.\n\n{app} is saving everything else it can.",
                skipped.join(", ")
            ),
            buttons: &["OK"],
        }
    }
}

fn pane_effect(app: &str, pane: &str) -> String {
    match pane {
        "Screen Recording" => {
            format!("{app} saves a screenshot at the end of each tracking interval.")
        }
        "Input Monitoring" => format!(
            "{app} counts key presses, clicks, and scrolling. It does not save what you type."
        ),
        "Accessibility" => {
            format!("{app} reads the browser address bar. It does not read the page.")
        }
        _ => format!("{app} needs this switch."),
    }
}

fn pane_name(capability: Capability) -> Option<&'static str> {
    match capability {
        Capability::ScreenCapture | Capability::VideoCapture => Some("Screen Recording"),
        Capability::InputMonitoring => Some("Input Monitoring"),
        Capability::BrowserUrl => Some("Accessibility"),
        Capability::ActiveApplication => None,
    }
}

fn settings_destination(capability: Capability) -> Option<&'static str> {
    match capability {
        Capability::ScreenCapture | Capability::VideoCapture => {
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        }
        Capability::InputMonitoring => {
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
        }
        Capability::BrowserUrl => {
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        }
        Capability::ActiveApplication => None,
    }
}

fn open_pane(capability: Capability) -> Result<(), ()> {
    let url = settings_destination(capability).ok_or(())?;
    open_system_settings(url)
}

fn access_granted(capability: Capability) -> bool {
    match capability {
        Capability::ScreenCapture | Capability::VideoCapture => {
            crate::screen_capture_permission().state == PermissionState::Granted
        }
        Capability::InputMonitoring => {
            crate::input_monitoring_permission().state == PermissionState::Granted
        }
        Capability::BrowserUrl => crate::url::accessibility_granted(),
        Capability::ActiveApplication => true,
    }
}

fn app_name(display_name: &str) -> String {
    let cleaned = display_name
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if cleaned.is_empty() {
        "the app".to_string()
    } else {
        cleaned
    }
}

enum GuideChoice {
    Done,
    OpenSettings,
    Reopen,
    Skip,
}

const CHOICE_SCRIPT: &str = r#"on run argv
set alertTitle to item 1 of argv
set alertMessage to item 2 of argv
set buttonNames to items 3 thru -1 of argv
display alert alertTitle message alertMessage as informational buttons buttonNames default button (last item of buttonNames)
end run"#;

fn present_choice(alert: &PermissionAlert) -> Result<GuideChoice, ()> {
    let mut command = Command::new("osascript");
    command
        .arg("-e")
        .arg(CHOICE_SCRIPT)
        .arg("--")
        .arg(&alert.title)
        .arg(&alert.message);
    for button in alert.buttons {
        command.arg(*button);
    }
    let child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| ())?;
    crate::set_permission_guide_pid(Some(child.id()));
    let output = child.wait_with_output().map_err(|_| ())?;
    crate::set_permission_guide_pid(None);
    if !output.status.success() {
        return Ok(GuideChoice::Skip);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let chosen = text
        .split("button returned:")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .map(str::trim)
        .unwrap_or("");
    if chosen == "Done" || chosen == "OK" {
        Ok(GuideChoice::Done)
    } else if chosen.contains("Open Settings") {
        Ok(GuideChoice::OpenSettings)
    } else if chosen.contains("Quit") {
        Ok(GuideChoice::Reopen)
    } else {
        Ok(GuideChoice::Skip)
    }
}

#[cfg(test)]
mod tests {
    use super::{finished_alert, still_needed_alert, turn_on_alert};
    use wind_core::Capability;

    #[test]
    fn the_prompt_stays_on_one_switch() {
        let alert = turn_on_alert("Northstar", "Screen Recording", 1, 3);
        assert!(alert.title.contains("Northstar"));
        assert!(alert.title.contains("1 of 3"));
        assert!(alert
            .message
            .contains("Settings is open to Screen Recording"));
        assert!(alert.message.contains("Turn on Northstar"));
        assert!(alert.message.contains("screenshot"));
        assert!(!alert.message.contains("Quit"));
        assert_eq!(alert.buttons, ["Skip", "Open Settings", "Done"]);
        assert!(super::pane_name(Capability::ActiveApplication).is_none());
    }

    #[test]
    fn a_failed_check_repeats_the_same_switch() {
        let alert = still_needed_alert("Northstar", "Input Monitoring");
        assert!(alert.title.contains("still off"));
        assert!(alert
            .message
            .contains("Privacy & Security → Input Monitoring"));
        assert!(alert.message.contains("does not save what you type"));
    }

    #[test]
    fn the_finish_note_lists_only_what_is_left() {
        let ready = finished_alert("Northstar", &[]);
        assert!(ready.title.contains("ready"));
        assert!(ready.message.contains("already saving"));
        let left = finished_alert("Northstar", &["Accessibility"]);
        assert!(left.message.contains("Still off: Accessibility"));
        assert!(left.message.contains("everything else"));
    }

    #[test]
    fn a_restart_permission_closes_so_macos_can_reopen_the_app() {
        let alert = super::relaunch_alert("Northstar", &["Screen Recording", "Input Monitoring"]);
        assert!(alert.message.contains("Quit & Reopen"));
        assert!(alert.message.contains("opens again by itself"));
        assert!(alert
            .message
            .contains("Screen Recording and Input Monitoring"));
        assert!(!alert.message.contains("Click Done"));
        assert_eq!(alert.buttons, ["Quit & Reopen"]);
    }

    #[test]
    fn an_empty_name_does_not_invent_a_brand() {
        let alert = turn_on_alert(&super::app_name("  \n"), "Accessibility", 2, 2);
        assert!(alert.message.contains("the app"));
        assert!(!alert.title.contains("Wind"));
        assert!(alert.message.contains("address bar"));
    }
}
