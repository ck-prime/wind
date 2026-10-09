//! OS-backed secret storage.
//!
//! Tracking services do not call this trait. Secrets never go into SQLite,
//! configuration files, or logs.

use std::collections::HashMap;
use std::sync::Mutex;

/// Protected secret storage. A missing name is `Ok(None)`.
///
/// An unavailable operating-system store is an error. Callers must not replace
/// that error with a plaintext file.
pub trait SecretStore: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, SecretError>;
    fn set(&self, name: &str, value: &[u8]) -> Result<(), SecretError>;
    fn delete(&self, name: &str) -> Result<(), SecretError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretError {
    Unavailable { reason: String },
    Rejected { reason: String },
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable { reason } => {
                write!(formatter, "secret store is unavailable: {reason}")
            }
            Self::Rejected { reason } => write!(formatter, "secret was rejected: {reason}"),
        }
    }
}

impl std::error::Error for SecretError {}

impl SecretError {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::Unavailable {
            reason: reason.into(),
        }
    }

    pub fn rejected(reason: impl Into<String>) -> Self {
        Self::Rejected {
            reason: reason.into(),
        }
    }
}

/// In-memory store for tests. It is not an operating-system fallback.
#[derive(Default)]
pub struct MemorySecretStore {
    values: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemorySecretStore {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, SecretError> {
        check_name(name)?;
        let values = self
            .values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok(values.get(name).cloned())
    }

    fn set(&self, name: &str, value: &[u8]) -> Result<(), SecretError> {
        check_name(name)?;
        if value.is_empty() {
            return Err(SecretError::rejected("secret value is empty"));
        }
        let mut values = self
            .values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        values.insert(name.to_string(), value.to_vec());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        check_name(name)?;
        let mut values = self
            .values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        values.remove(name);
        Ok(())
    }
}

pub fn check_name(name: &str) -> Result<(), SecretError> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(SecretError::rejected("secret name is not accepted"));
    }
    Ok(())
}

#[cfg(test)]
struct UnavailableStore;

#[cfg(test)]
impl SecretStore for UnavailableStore {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, SecretError> {
        check_name(name)?;
        crate::report_secret_unavailable();
        Err(SecretError::unavailable("secure store is unavailable"))
    }

    fn set(&self, name: &str, _value: &[u8]) -> Result<(), SecretError> {
        check_name(name)?;
        crate::report_secret_unavailable();
        Err(SecretError::unavailable("secure store is unavailable"))
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        check_name(name)?;
        crate::report_secret_unavailable();
        Err(SecretError::unavailable("secure store is unavailable"))
    }
}

#[cfg(test)]
mod tests {
    use super::{MemorySecretStore, SecretStore, UnavailableStore};
    use crate::{SqliteStore, StoreOptions};
    use std::fs;

    fn options() -> StoreOptions {
        StoreOptions {
            max_local_storage_bytes: 1024 * 1024,
            config_version: "0.1.0".to_string(),
            user_id: None,
            sanitize_url: |_| Err("url was rejected".to_string()),
        }
    }

    #[test]
    fn memory_secret_round_trip_does_not_enter_sqlite() {
        let secret = b"phase8-memory-secret";
        let memory = MemorySecretStore::new();
        memory.set("phase8_memory", secret).expect("set");
        let loaded = memory.get("phase8_memory").expect("get");
        assert!(loaded.as_deref() == Some(secret.as_slice()));
        assert!(memory.get("missing").expect("missing").is_none());
        memory.delete("phase8_memory").expect("delete");
        assert!(memory.get("phase8_memory").expect("deleted").is_none());

        let directory =
            std::env::temp_dir().join(format!("wind-storage-secret-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("dir");
        let store = SqliteStore::open(&directory, options()).expect("open");
        let bytes = fs::read(store.database_path()).expect("db");
        assert!(!bytes.windows(secret.len()).any(|window| window == secret));
        drop(store);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn unavailable_store_returns_an_error() {
        let store = UnavailableStore;
        let error = store
            .set("phase8_closed", b"unused")
            .expect_err("unavailable");
        assert!(error.to_string().contains("unavailable"));
        assert!(store.get("phase8_closed").is_err());
    }
}
