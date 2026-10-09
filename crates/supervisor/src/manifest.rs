//! Installed service metadata.
//!
//! The manifest records which executable version is installed. It is not a
//! permission database, a capability answer, or the tracking database.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use wind_core::{Error, ServiceId, Version, SERVICE_PROTOCOL_VERSION};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceInstall {
    pub id: ServiceId,
    pub version: Version,
    pub protocol_version: u32,
    pub executable: PathBuf,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceManifest {
    services: Vec<ServiceInstall>,
}

#[derive(Debug, Deserialize)]
struct ManifestFile {
    #[serde(default)]
    service: Vec<ManifestEntry>,
}

#[derive(Debug, Deserialize)]
struct ManifestEntry {
    id: String,
    version: String,
    protocol_version: u32,
    path: String,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

impl ServiceManifest {
    pub fn empty() -> Self {
        Self {
            services: Vec::new(),
        }
    }

    pub fn install(&self, id: ServiceId) -> Option<&ServiceInstall> {
        self.services.iter().find(|service| service.id == id)
    }

    /// Load an explicit manifest path, or the runtime file when it exists.
    ///
    /// `WIND_SERVICE_MANIFEST` selects a manifest for a launch. A missing
    /// runtime file means every service stays in-process.
    pub fn load(runtime_dir: &Path) -> Result<Self, Error> {
        if let Some(path) = std::env::var_os("WIND_SERVICE_MANIFEST") {
            return Self::read(Path::new(&path));
        }
        let path = runtime_dir.join("services.toml");
        if !path.exists() {
            return Ok(Self::empty());
        }
        Self::read(&path)
    }

    pub fn read(path: &Path) -> Result<Self, Error> {
        let text = fs::read_to_string(path).map_err(|_| {
            Error::new(
                "supervisor",
                "read_manifest",
                "service manifest could not be read",
            )
        })?;
        let parsed: ManifestFile = toml::from_str(&text).map_err(|_| {
            Error::new(
                "supervisor",
                "read_manifest",
                "service manifest could not be parsed",
            )
        })?;
        let mut services = Vec::new();
        for entry in parsed.service {
            let id = ServiceId::parse(&entry.id).ok_or_else(|| {
                Error::new(
                    "supervisor",
                    "read_manifest",
                    format!("unknown service id {}", entry.id),
                )
            })?;
            if services
                .iter()
                .any(|service: &ServiceInstall| service.id == id)
            {
                return Err(Error::new(
                    "supervisor",
                    "read_manifest",
                    format!("duplicate service id {}", id.as_str()),
                ));
            }
            let version = Version::parse(&entry.version).map_err(|_| {
                Error::new(
                    "supervisor",
                    "read_manifest",
                    "service version must be major.minor.patch",
                )
            })?;
            if entry.protocol_version != SERVICE_PROTOCOL_VERSION {
                return Err(Error::new(
                    "supervisor",
                    "read_manifest",
                    format!(
                        "Incompatible service protocol expected: {SERVICE_PROTOCOL_VERSION} received: {}",
                        entry.protocol_version
                    ),
                )
                .with_service(id.as_str()));
            }
            services.push(ServiceInstall {
                id,
                version,
                protocol_version: entry.protocol_version,
                executable: PathBuf::from(entry.path),
                enabled: entry.enabled,
            });
        }
        Ok(Self { services })
    }
}

#[cfg(test)]
mod tests {
    use super::ServiceManifest;
    use std::io::Write;

    #[test]
    fn incompatible_manifest_is_rejected() {
        let path = std::env::temp_dir().join(format!("wind-manifest-{}.toml", std::process::id()));
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(
            file,
            "[[service]]\nid = \"app_tracker\"\nversion = \"1.0.0\"\nprotocol_version = 2\npath = \"C:/wind-app-tracker.exe\"\n"
        )
        .unwrap();
        let error = ServiceManifest::read(&path).unwrap_err();
        assert!(error.to_string().contains("expected: 1"), "{error}");
        assert!(error.to_string().contains("received: 2"), "{error}");
        let _ = std::fs::remove_file(path);
    }
}
