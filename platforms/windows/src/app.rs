//! Foreground application identity.
//!
//! The foreground window identifies a process. The process is opened with
//! `PROCESS_QUERY_LIMITED_INFORMATION` and the executable path is read with
//! `QueryFullProcessImageNameW`. The window title is not read. The process
//! handle is closed before the function returns.

use std::path::Path;
use std::time::SystemTime;

use wind_app_tracker::{ActiveApplication, AppError};
use wind_core::CapabilityStatus;
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

pub fn application_status() -> CapabilityStatus {
    CapabilityStatus::Available
}

pub(crate) struct ForegroundProcess {
    pub window: HWND,
    pub application: ActiveApplication,
}

pub(crate) fn foreground_process() -> Result<Option<ForegroundProcess>, AppError> {
    let window = unsafe { GetForegroundWindow() };
    if window.is_invalid() {
        return Ok(None);
    }
    let mut pid = 0u32;
    let thread = unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    if thread == 0 || pid == 0 {
        return Err(AppError::QueryFailed {
            reason: "foreground window has no process id".to_string(),
        });
    }
    let handle =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.map_err(|error| {
            if error.code().0 as u32 == 0x8007_0005 {
                AppError::Unavailable {
                    reason: "foreground process does not allow a limited query".to_string(),
                }
            } else {
                AppError::QueryFailed {
                    reason: error.to_string(),
                }
            }
        })?;
    let owned = ProcessHandle(handle);
    let path = image_path(owned.0)?;
    drop(owned);
    let application_id = normalize_id(&path);
    let name = Path::new(&path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .map(str::to_string);
    if application_id.is_empty() {
        return Err(AppError::QueryFailed {
            reason: "foreground executable path is empty".to_string(),
        });
    }
    Ok(Some(ForegroundProcess {
        window,
        application: ActiveApplication {
            application_id,
            name,
            executable: Some(path),
            observed_at: SystemTime::now(),
        },
    }))
}

pub fn current_application() -> Result<Option<ActiveApplication>, AppError> {
    Ok(foreground_process()?.map(|foreground| foreground.application))
}

fn image_path(process: HANDLE) -> Result<String, AppError> {
    let mut capacity = 1024u32;
    let mut buffer = vec![0u16; capacity as usize];
    let status = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut capacity,
        )
    };
    if status.is_err() {
        capacity = 32_768;
        buffer.resize(capacity as usize, 0);
        unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut capacity,
            )
        }
        .map_err(|error| AppError::QueryFailed {
            reason: error.to_string(),
        })?;
    }
    let length = capacity as usize;
    let text = String::from_utf16(buffer.get(..length).unwrap_or(&[])).map_err(|_| {
        AppError::QueryFailed {
            reason: "foreground executable path is not unicode".to_string(),
        }
    })?;
    Ok(text.trim_end_matches('\0').to_string())
}

fn normalize_id(path: &str) -> String {
    path.strip_prefix(r"\\?\").unwrap_or(path).to_lowercase()
}

struct ProcessHandle(HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::current_application;

    #[test]
    fn foreground_application_has_an_executable_identity() {
        let application = current_application()
            .expect("query")
            .expect("a foreground window");
        assert!(
            application
                .executable
                .as_deref()
                .unwrap_or("")
                .contains('\\'),
            "missing executable"
        );
        assert_eq!(
            application.application_id,
            application
                .executable
                .as_deref()
                .unwrap_or("")
                .trim_start_matches(r"\\?\")
                .to_lowercase()
        );
        assert!(!application.name.as_deref().unwrap_or("").is_empty());
        assert!(!application.application_id.contains('\n'));
        eprintln!(
            "windows foreground name={} id_len={}",
            application.name.as_deref().unwrap_or("-"),
            application.application_id.len()
        );
    }
}
