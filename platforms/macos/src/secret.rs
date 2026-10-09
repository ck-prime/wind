//! macOS Keychain.
//!
//! Generic passwords are stored for the current user. This file is not
//! compiled or runtime-tested by a Windows build.

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::os::raw::{c_char, c_uint};

use wind_storage::{check_name, report_secret_unavailable, SecretError, SecretStore};

const SERVICE: &str = "com.wind.agent";
const ERR_SUCCESS: i32 = 0;
const ERR_NOT_FOUND: i32 = -25300;

#[link(name = "Security", kind = "framework")]
extern "C" {
    fn SecKeychainAddGenericPassword(
        keychain: *mut c_void,
        service_name_length: c_uint,
        service_name: *const c_char,
        account_name_length: c_uint,
        account_name: *const c_char,
        password_length: c_uint,
        password_data: *const c_void,
        item_ref: *mut *mut c_void,
    ) -> i32;
    fn SecKeychainFindGenericPassword(
        keychain: *mut c_void,
        service_name_length: c_uint,
        service_name: *const c_char,
        account_name_length: c_uint,
        account_name: *const c_char,
        password_length: *mut c_uint,
        password_data: *mut *mut c_void,
        item_ref: *mut *mut c_void,
    ) -> i32;
    fn SecKeychainItemDelete(item: *mut c_void) -> i32;
    fn SecKeychainItemFreeContent(attribute_list: *mut c_void, data: *mut c_void) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(value: *const c_void);
}

/// Current-user Keychain store.
#[derive(Debug, Default)]
pub struct MacosSecretStore;

impl SecretStore for MacosSecretStore {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, SecretError> {
        check_name(name)?;
        unsafe {
            match find(name) {
                Find::Missing => Ok(None),
                Find::Failed => {
                    report_secret_unavailable();
                    Err(SecretError::unavailable("macOS Keychain could not be read"))
                }
                Find::Found { bytes, data, item } => {
                    let value = bytes.to_vec();
                    if !data.is_null() {
                        let _ = SecKeychainItemFreeContent(std::ptr::null_mut(), data);
                    }
                    if !item.is_null() {
                        CFRelease(item);
                    }
                    Ok(Some(value))
                }
            }
        }
    }

    fn set(&self, name: &str, value: &[u8]) -> Result<(), SecretError> {
        check_name(name)?;
        if value.is_empty() {
            return Err(SecretError::rejected("secret value is empty"));
        }
        self.delete(name)?;
        let length = c_uint::try_from(value.len())
            .map_err(|_| SecretError::rejected("secret value is too large"))?;
        let status = unsafe {
            SecKeychainAddGenericPassword(
                std::ptr::null_mut(),
                SERVICE.len() as c_uint,
                SERVICE.as_ptr() as *const c_char,
                name.len() as c_uint,
                name.as_ptr() as *const c_char,
                length,
                value.as_ptr() as *const c_void,
                std::ptr::null_mut(),
            )
        };
        if status == ERR_SUCCESS {
            Ok(())
        } else {
            report_secret_unavailable();
            Err(SecretError::unavailable(
                "macOS Keychain could not be written",
            ))
        }
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        check_name(name)?;
        unsafe {
            match find(name) {
                Find::Missing => Ok(()),
                Find::Failed => {
                    report_secret_unavailable();
                    Err(SecretError::unavailable(
                        "macOS Keychain could not be updated",
                    ))
                }
                Find::Found { data, item, .. } => {
                    if !data.is_null() {
                        let _ = SecKeychainItemFreeContent(std::ptr::null_mut(), data);
                    }
                    let status = if item.is_null() {
                        ERR_SUCCESS
                    } else {
                        let status = SecKeychainItemDelete(item);
                        CFRelease(item);
                        status
                    };
                    if status == ERR_SUCCESS || status == ERR_NOT_FOUND {
                        Ok(())
                    } else {
                        report_secret_unavailable();
                        Err(SecretError::unavailable(
                            "macOS Keychain could not be updated",
                        ))
                    }
                }
            }
        }
    }
}

enum Find<'a> {
    Missing,
    Failed,
    Found {
        bytes: &'a [u8],
        data: *mut c_void,
        item: *mut c_void,
    },
}

unsafe fn find(name: &str) -> Find<'_> {
    let mut length = 0;
    let mut data = std::ptr::null_mut();
    let mut item = std::ptr::null_mut();
    let status = SecKeychainFindGenericPassword(
        std::ptr::null_mut(),
        SERVICE.len() as c_uint,
        SERVICE.as_ptr() as *const c_char,
        name.len() as c_uint,
        name.as_ptr() as *const c_char,
        &mut length,
        &mut data,
        &mut item,
    );
    if status == ERR_NOT_FOUND {
        return Find::Missing;
    }
    if status != ERR_SUCCESS || data.is_null() {
        if !item.is_null() {
            CFRelease(item);
        }
        return Find::Failed;
    }
    Find::Found {
        bytes: std::slice::from_raw_parts(data as *const u8, length as usize),
        data,
        item,
    }
}
