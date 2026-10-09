//! Windows Credential Manager.
//!
//! Credentials are stored for the current user. This does not require an
//! administrator account and does not write the secret to a file.

use std::ffi::c_void;

use wind_storage::{check_name, report_secret_unavailable, SecretError, SecretStore};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_NOT_FOUND, FILETIME};
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_FLAGS,
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
};

const PREFIX: &str = "com.wind.agent/";

/// Current-user generic credential store.
#[derive(Debug, Default)]
pub struct WindowsSecretStore;

impl SecretStore for WindowsSecretStore {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, SecretError> {
        check_name(name)?;
        let target = wide_target(name)?;
        unsafe {
            let mut credential = std::ptr::null_mut();
            match CredReadW(
                PCWSTR(target.as_ptr()),
                CRED_TYPE_GENERIC,
                None,
                &mut credential,
            ) {
                Ok(()) => {
                    if credential.is_null() {
                        return Ok(None);
                    }
                    let header = &*credential;
                    let value = std::slice::from_raw_parts(
                        header.CredentialBlob,
                        header.CredentialBlobSize as usize,
                    )
                    .to_vec();
                    CredFree(credential as *const c_void);
                    Ok(Some(value))
                }
                Err(error) if error.code() == ERROR_NOT_FOUND.into() => Ok(None),
                Err(_) => {
                    report_secret_unavailable();
                    Err(SecretError::unavailable(
                        "Windows Credential Manager could not be read",
                    ))
                }
            }
        }
    }

    fn set(&self, name: &str, value: &[u8]) -> Result<(), SecretError> {
        check_name(name)?;
        if value.is_empty() {
            return Err(SecretError::rejected("secret value is empty"));
        }
        let size = u32::try_from(value.len())
            .map_err(|_| SecretError::rejected("secret value is too large"))?;
        let mut target = wide_target(name)?;
        let mut blob = value.to_vec();
        let credential = CREDENTIALW {
            Flags: CRED_FLAGS(0),
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target.as_mut_ptr()),
            Comment: PWSTR::null(),
            LastWritten: FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            },
            CredentialBlobSize: size,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: std::ptr::null_mut(),
            TargetAlias: PWSTR::null(),
            UserName: PWSTR::null(),
        };
        unsafe {
            CredWriteW(&credential, 0).map_err(|_| {
                report_secret_unavailable();
                SecretError::unavailable("Windows Credential Manager could not be written")
            })
        }
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        check_name(name)?;
        let target = wide_target(name)?;
        unsafe {
            match CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) {
                Ok(()) => Ok(()),
                Err(error) if error.code() == ERROR_NOT_FOUND.into() => Ok(()),
                Err(_) => {
                    report_secret_unavailable();
                    Err(SecretError::unavailable(
                        "Windows Credential Manager could not be updated",
                    ))
                }
            }
        }
    }
}

fn wide_target(name: &str) -> Result<Vec<u16>, SecretError> {
    let target = format!("{PREFIX}{name}");
    let mut wide: Vec<u16> = target.encode_utf16().collect();
    wide.push(0);
    Ok(wide)
}

#[cfg(test)]
mod tests {
    use super::WindowsSecretStore;
    use wind_storage::SecretStore;

    #[test]
    fn credential_manager_round_trip_does_not_print_the_secret() {
        let store = WindowsSecretStore;
        let name = "phase8test";
        let value = b"phase8-credential-value";
        store.delete(name).expect("clear");
        store.set(name, value).expect("set");
        let loaded = store.get(name).expect("get");
        assert!(loaded.as_deref() == Some(value.as_slice()));
        store.delete(name).expect("delete");
        assert!(store.get(name).expect("missing").is_none());
    }
}
