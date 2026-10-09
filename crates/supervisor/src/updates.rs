//! Local service update staging.
//!
//! Packages are verified before they are activated. This module does not
//! download, trust a file name, or embed a production signing key. A failed
//! health check restores the previous version directory. The new directory is
//! kept so the failure can be inspected. User data is not touched.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use wind_core::{ServiceId, Version, SERVICE_PROTOCOL_VERSION};
use wind_platform::service_executable_name;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageMetadata {
    pub service_id: ServiceId,
    pub version: Version,
    pub protocol_version: u32,
    pub platform: String,
    pub architecture: String,
    pub sha256: [u8; 32],
    pub signature: Vec<u8>,
}

#[derive(Debug)]
pub enum VerifyError {
    Digest,
    Signature,
    Protocol { expected: u32, received: u32 },
    Platform,
    Identity,
    Untrusted,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Digest => formatter.write_str("service package digest does not match"),
            Self::Signature => formatter.write_str("service package signature was rejected"),
            Self::Protocol { expected, received } => write!(
                formatter,
                "Incompatible service protocol expected: {expected} received: {received}"
            ),
            Self::Platform => formatter.write_str("service package platform does not match"),
            Self::Identity => formatter.write_str("service package identity was rejected"),
            Self::Untrusted => {
                formatter.write_str("service package has no trusted publisher signature")
            }
        }
    }
}

impl std::error::Error for VerifyError {}

pub trait PackageVerifier {
    fn verify(&self, metadata: &PackageMetadata, bytes: &[u8]) -> Result<(), VerifyError>;
}

/// Production verifier. It rejects every package until a publisher key exists.
#[derive(Clone, Copy, Debug)]
pub struct ClosedVerifier;

impl PackageVerifier for ClosedVerifier {
    fn verify(&self, _metadata: &PackageMetadata, _bytes: &[u8]) -> Result<(), VerifyError> {
        Err(VerifyError::Untrusted)
    }
}

/// Test fixture. The signature must equal the SHA-256 digest. This is not a key.
#[derive(Clone, Copy, Debug)]
pub struct FixtureVerifier {
    pub platform: &'static str,
    pub architecture: &'static str,
}

impl PackageVerifier for FixtureVerifier {
    fn verify(&self, metadata: &PackageMetadata, bytes: &[u8]) -> Result<(), VerifyError> {
        if metadata.protocol_version != SERVICE_PROTOCOL_VERSION {
            return Err(VerifyError::Protocol {
                expected: SERVICE_PROTOCOL_VERSION,
                received: metadata.protocol_version,
            });
        }
        if metadata.platform != self.platform || metadata.architecture != self.architecture {
            return Err(VerifyError::Platform);
        }
        if digest(bytes) != metadata.sha256 {
            return Err(VerifyError::Digest);
        }
        if metadata.signature.as_slice() != metadata.sha256 {
            return Err(VerifyError::Signature);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum UpdateError {
    Verify(VerifyError),
    Io(String),
    MissingPrevious,
    AlreadyPresent,
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Verify(error) => write!(formatter, "{error}"),
            Self::Io(reason) => formatter.write_str(reason),
            Self::MissingPrevious => {
                formatter.write_str("no previous service version is installed")
            }
            Self::AlreadyPresent => formatter.write_str("service version directory already exists"),
        }
    }
}

impl std::error::Error for UpdateError {}

pub struct UpdateManager<V> {
    root: PathBuf,
    verifier: V,
}

impl<V: PackageVerifier> UpdateManager<V> {
    pub fn new(root: PathBuf, verifier: V) -> Self {
        Self { root, verifier }
    }

    pub fn stage(&self, metadata: &PackageMetadata, bytes: &[u8]) -> Result<PathBuf, UpdateError> {
        self.verifier
            .verify(metadata, bytes)
            .map_err(UpdateError::Verify)?;
        let directory = self.version_dir(metadata.service_id, metadata.version);
        if directory.exists() {
            return Err(UpdateError::AlreadyPresent);
        }
        fs::create_dir_all(&directory)
            .map_err(|_| UpdateError::Io("stage directory failed".into()))?;
        let destination = directory.join(service_executable_name(metadata.service_id));
        let temporary = directory.join("package.partial");
        fs::write(&temporary, bytes).map_err(|_| UpdateError::Io("package write failed".into()))?;
        fs::rename(&temporary, &destination)
            .map_err(|_| UpdateError::Io("package activation failed".into()))?;
        Ok(destination)
    }

    pub fn activate(&self, id: ServiceId, version: Version) -> Result<(), UpdateError> {
        let directory = self.version_dir(id, version);
        if !directory.is_dir() {
            return Err(UpdateError::Io("staged service version is missing".into()));
        }
        let active_path = self.pointer(id, "active");
        if let Some(parent) = active_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|_| UpdateError::Io("service root failed".into()))?;
        }
        if let Ok(current) = fs::read_to_string(&active_path) {
            let current = current.trim();
            if !current.is_empty() && current != version.to_string() {
                fs::write(self.pointer(id, "previous"), current)
                    .map_err(|_| UpdateError::Io("previous version could not be saved".into()))?;
            }
        }
        fs::write(&active_path, format!("{version}\n"))
            .map_err(|_| UpdateError::Io("active version could not be saved".into()))?;
        Ok(())
    }

    pub fn active(&self, id: ServiceId) -> Result<Version, UpdateError> {
        let text = fs::read_to_string(self.pointer(id, "active"))
            .map_err(|_| UpdateError::Io("active version could not be read".into()))?;
        Version::parse(text.trim()).map_err(|_| UpdateError::Io("active version is invalid".into()))
    }

    pub fn rollback(&self, id: ServiceId) -> Result<Version, UpdateError> {
        let previous = fs::read_to_string(self.pointer(id, "previous"))
            .map_err(|_| UpdateError::MissingPrevious)?;
        let version = Version::parse(previous.trim()).map_err(|_| UpdateError::MissingPrevious)?;
        self.activate(id, version)?;
        Ok(version)
    }

    /// Activate `version` and keep it only when `health` reports success.
    pub fn commit<F>(
        &self,
        id: ServiceId,
        version: Version,
        health: F,
    ) -> Result<Version, UpdateError>
    where
        F: FnOnce(&Path) -> bool,
    {
        self.activate(id, version)?;
        let executable = self
            .version_dir(id, version)
            .join(service_executable_name(id));
        if health(&executable) {
            Ok(version)
        } else {
            self.rollback(id)
        }
    }

    pub fn version_dir(&self, id: ServiceId, version: Version) -> PathBuf {
        self.root
            .join(id.technical_component())
            .join(version.to_string())
    }

    fn pointer(&self, id: ServiceId, name: &str) -> PathBuf {
        self.root.join(id.technical_component()).join(name)
    }
}

pub fn digest(bytes: &[u8]) -> [u8; 32] {
    let hashed = Sha256::digest(bytes);
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&hashed);
    digest
}

#[cfg(test)]
mod tests {
    use super::{digest, FixtureVerifier, PackageMetadata, UpdateError, UpdateManager};
    use wind_core::{ServiceId, Version};

    fn metadata(id: ServiceId, version: Version, bytes: &[u8], protocol: u32) -> PackageMetadata {
        let sha256 = digest(bytes);
        PackageMetadata {
            service_id: id,
            version,
            protocol_version: protocol,
            platform: "test".to_string(),
            architecture: "test".to_string(),
            signature: sha256.to_vec(),
            sha256,
        }
    }

    fn manager(root: &std::path::Path) -> UpdateManager<FixtureVerifier> {
        UpdateManager::new(
            root.to_path_buf(),
            FixtureVerifier {
                platform: "test",
                architecture: "test",
            },
        )
    }

    #[test]
    fn failed_update_rolls_back_and_keeps_both_versions() {
        let root = std::env::temp_dir().join(format!("wind-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let updates = manager(&root);
        let first = metadata(ServiceId::Screenshot, Version::new(1, 0, 0), b"v1", 1);
        let second = metadata(ServiceId::Screenshot, Version::new(1, 1, 0), b"v2", 1);
        updates.stage(&first, b"v1").unwrap();
        updates
            .activate(ServiceId::Screenshot, Version::new(1, 0, 0))
            .unwrap();
        updates.stage(&second, b"v2").unwrap();
        let restored = updates
            .commit(ServiceId::Screenshot, Version::new(1, 1, 0), |_| false)
            .unwrap();
        assert_eq!(restored, Version::new(1, 0, 0));
        assert_eq!(
            updates.active(ServiceId::Screenshot).unwrap(),
            Version::new(1, 0, 0)
        );
        assert!(updates
            .version_dir(ServiceId::Screenshot, Version::new(1, 0, 0))
            .exists());
        assert!(updates
            .version_dir(ServiceId::Screenshot, Version::new(1, 1, 0))
            .exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unsigned_or_incompatible_packages_are_rejected() {
        let root = std::env::temp_dir().join(format!("wind-update-reject-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let updates = manager(&root);
        let mut bad = metadata(ServiceId::Activity, Version::new(1, 0, 0), b"bytes", 1);
        bad.signature = b"filename".to_vec();
        assert!(matches!(
            updates.stage(&bad, b"bytes").unwrap_err(),
            UpdateError::Verify(_)
        ));
        let incompatible = metadata(ServiceId::Activity, Version::new(2, 0, 0), b"next", 2);
        let error = updates.stage(&incompatible, b"next").unwrap_err();
        assert!(error.to_string().contains("expected: 1"));
        assert!(error.to_string().contains("received: 2"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn services_keep_independent_versions() {
        let root = std::env::temp_dir().join(format!("wind-versions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let updates = manager(&root);
        let installed = [
            (ServiceId::Screenshot, Version::new(1, 1, 0)),
            (ServiceId::Activity, Version::new(1, 0, 0)),
            (ServiceId::AppTracker, Version::new(1, 2, 0)),
            (ServiceId::UrlTracker, Version::new(1, 0, 0)),
            (ServiceId::Video, Version::new(1, 0, 0)),
        ];
        for (id, version) in installed {
            let bytes = version.to_string();
            let package = metadata(id, version, bytes.as_bytes(), 1);
            updates.stage(&package, bytes.as_bytes()).unwrap();
            updates.activate(id, version).unwrap();
            assert_eq!(updates.active(id).unwrap(), version);
        }
        assert_ne!(
            updates.active(ServiceId::Screenshot).unwrap(),
            updates.active(ServiceId::Activity).unwrap()
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
