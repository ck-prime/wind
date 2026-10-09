//! User-scoped runtime directories.
//!
//! Path construction is pure. The host lookup reads environment variables and
//! does not depend on the process working directory.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// Directories the resident runtime may create for one user.
///
/// Every path is absolute. None of them are `./config`, `./logs`, or `./data`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub log_dir: PathBuf,
    pub runtime_dir: PathBuf,
}

impl RuntimePaths {
    pub fn directories(&self) -> [(&'static str, &Path); 5] {
        [
            ("config", &self.config_dir),
            ("data", &self.data_dir),
            ("cache", &self.cache_dir),
            ("log", &self.log_dir),
            ("runtime", &self.runtime_dir),
        ]
    }
}

/// Inputs for the XDG base-directory layout.
///
/// Relative XDG values are ignored. A missing value uses the standard fallback
/// under `home`.
#[derive(Debug, Clone, Copy)]
pub struct LinuxPathEnv<'a> {
    pub home: &'a Path,
    pub config_home: Option<&'a Path>,
    pub data_home: Option<&'a Path>,
    pub cache_home: Option<&'a Path>,
    pub state_home: Option<&'a Path>,
    pub runtime_dir: Option<&'a Path>,
}

/// Failure while resolving or creating runtime directories, or while installing
/// the termination notifier.
#[derive(Debug)]
pub enum PlatformError {
    InvalidIdentifier {
        reason: String,
    },
    MissingLocation {
        variable: &'static str,
    },
    RelativePath {
        role: &'static str,
        path: PathBuf,
    },
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Signal {
        reason: String,
    },
}

impl fmt::Display for PlatformError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentifier { reason } => {
                write!(formatter, "invalid product identifier: {reason}")
            }
            Self::MissingLocation { variable } => {
                write!(formatter, "runtime path requires {variable}")
            }
            Self::RelativePath { role, path } => {
                write!(
                    formatter,
                    "runtime {role} path must be absolute, got {}",
                    path.display()
                )
            }
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "failed to {operation} {}: {source}",
                path.display()
            ),
            Self::Signal { reason } => write!(formatter, "termination handler: {reason}"),
        }
    }
}

impl std::error::Error for PlatformError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidIdentifier { .. }
            | Self::MissingLocation { .. }
            | Self::RelativePath { .. }
            | Self::Signal { .. } => None,
        }
    }
}

/// macOS per-user application directories under `home`.
pub fn macos_runtime_paths(
    home: &Path,
    product_identifier: &str,
) -> Result<RuntimePaths, PlatformError> {
    require_identifier(product_identifier)?;
    let support = join_absolute(
        home,
        "home",
        &["Library", "Application Support", product_identifier],
    )?;
    Ok(RuntimePaths {
        config_dir: support.join("config"),
        data_dir: support.join("data"),
        cache_dir: join_absolute(home, "home", &["Library", "Caches", product_identifier])?,
        log_dir: join_absolute(home, "home", &["Library", "Logs", product_identifier])?,
        runtime_dir: support.join("runtime"),
    })
}

/// Windows per-user directories.
///
/// Configuration uses the roaming profile. Data, cache, logs, and runtime
/// files use the local profile. Neither path is a protected system directory.
pub fn windows_runtime_paths(
    roaming: &Path,
    local: &Path,
    product_identifier: &str,
) -> Result<RuntimePaths, PlatformError> {
    require_identifier(product_identifier)?;
    Ok(RuntimePaths {
        config_dir: join_absolute(roaming, "roaming", &[product_identifier, "config"])?,
        data_dir: join_absolute(local, "local", &[product_identifier, "data"])?,
        cache_dir: join_absolute(local, "local", &[product_identifier, "cache"])?,
        log_dir: join_absolute(local, "local", &[product_identifier, "logs"])?,
        runtime_dir: join_absolute(local, "local", &[product_identifier, "runtime"])?,
    })
}

/// Linux XDG directories with home-directory fallbacks.
///
/// `XDG_RUNTIME_DIR` is used only when it is absolute. Otherwise the runtime
/// directory is under the state home, which does not require root.
pub fn linux_runtime_paths(
    env: &LinuxPathEnv<'_>,
    product_identifier: &str,
) -> Result<RuntimePaths, PlatformError> {
    require_identifier(product_identifier)?;
    let config_root = absolute_or_default(env.config_home, env.home, &[".config"])?;
    let data_root = absolute_or_default(env.data_home, env.home, &[".local", "share"])?;
    let cache_root = absolute_or_default(env.cache_home, env.home, &[".cache"])?;
    let state_root = absolute_or_default(env.state_home, env.home, &[".local", "state"])?;
    let runtime_dir = if let Some(path) = absolute_optional(env.runtime_dir) {
        path.join(product_identifier)
    } else {
        state_root.join(product_identifier).join("runtime")
    };
    Ok(RuntimePaths {
        config_dir: config_root.join(product_identifier),
        data_dir: data_root.join(product_identifier),
        cache_dir: cache_root.join(product_identifier),
        log_dir: state_root.join(product_identifier).join("log"),
        runtime_dir,
    })
}

/// Resolve directories for the operating system this binary was built for.
pub fn host_runtime_paths(product_identifier: &str) -> Result<RuntimePaths, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        macos_host_paths(product_identifier)
    }
    #[cfg(target_os = "windows")]
    {
        windows_host_paths(product_identifier)
    }
    #[cfg(target_os = "linux")]
    {
        linux_host_paths(product_identifier)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = product_identifier;
        Err(PlatformError::MissingLocation {
            variable: "a supported operating system home directory",
        })
    }
}

/// Create each runtime directory. Existing directories are left in place.
pub fn ensure_runtime_paths(paths: &RuntimePaths) -> Result<(), PlatformError> {
    for (role, path) in paths.directories() {
        if !path.is_absolute() {
            return Err(PlatformError::RelativePath {
                role,
                path: path.to_path_buf(),
            });
        }
        std::fs::create_dir_all(path).map_err(|source| PlatformError::Io {
            operation: "create",
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_host_paths(product_identifier: &str) -> Result<RuntimePaths, PlatformError> {
    let home = required_env_path("HOME")?;
    let runtime_root = runtime_root_override(std::env::var_os("WIND_RUNTIME_ROOT"))?;
    macos_paths(&home, runtime_root.as_deref(), product_identifier)
}

/// macOS paths for a home directory, or for `WIND_RUNTIME_ROOT` when it is set.
///
/// The override is the home anchor. Library paths stay under that root, so a
/// test profile does not write the user's normal Application Support directory.
#[cfg(target_os = "macos")]
fn macos_paths(
    home: &Path,
    runtime_root: Option<&Path>,
    product_identifier: &str,
) -> Result<RuntimePaths, PlatformError> {
    macos_runtime_paths(runtime_root.unwrap_or(home), product_identifier)
}

#[cfg(target_os = "windows")]
fn windows_host_paths(product_identifier: &str) -> Result<RuntimePaths, PlatformError> {
    if let Some(root) = runtime_root_override(std::env::var_os("WIND_RUNTIME_ROOT"))? {
        return windows_runtime_paths(&root, &root, product_identifier);
    }
    let profile = env_path("USERPROFILE");
    let roaming = env_path("APPDATA")
        .or_else(|| {
            profile
                .as_ref()
                .map(|home| home.join("AppData").join("Roaming"))
        })
        .ok_or(PlatformError::MissingLocation {
            variable: "APPDATA",
        })?;
    let local = env_path("LOCALAPPDATA")
        .or_else(|| {
            profile
                .as_ref()
                .map(|home| home.join("AppData").join("Local"))
        })
        .unwrap_or_else(|| roaming.clone());
    windows_runtime_paths(&roaming, &local, product_identifier)
}

#[cfg(target_os = "linux")]
fn linux_host_paths(product_identifier: &str) -> Result<RuntimePaths, PlatformError> {
    let home = required_env_path("HOME")?;
    let config_home = env_path("XDG_CONFIG_HOME");
    let data_home = env_path("XDG_DATA_HOME");
    let cache_home = env_path("XDG_CACHE_HOME");
    let state_home = env_path("XDG_STATE_HOME");
    let runtime_dir = env_path("XDG_RUNTIME_DIR");
    linux_runtime_paths(
        &LinuxPathEnv {
            home: &home,
            config_home: config_home.as_deref(),
            data_home: data_home.as_deref(),
            cache_home: cache_home.as_deref(),
            state_home: state_home.as_deref(),
            runtime_dir: runtime_dir.as_deref(),
        },
        product_identifier,
    )
}

fn require_identifier(identifier: &str) -> Result<(), PlatformError> {
    if identifier.is_empty()
        || identifier == "."
        || identifier == ".."
        || identifier.contains("..")
        || identifier
            .bytes()
            .any(|byte| matches!(byte, b'/' | b'\\' | b':' | 0))
    {
        return Err(PlatformError::InvalidIdentifier {
            reason: "must be a single directory name without path separators".to_string(),
        });
    }
    Ok(())
}

fn join_absolute(
    anchor: &Path,
    role: &'static str,
    parts: &[&str],
) -> Result<PathBuf, PlatformError> {
    if !anchor.is_absolute() {
        return Err(PlatformError::RelativePath {
            role,
            path: anchor.to_path_buf(),
        });
    }
    let mut path = anchor.to_path_buf();
    for part in parts {
        path.push(part);
    }
    Ok(path)
}

fn absolute_optional(path: Option<&Path>) -> Option<&Path> {
    path.filter(|path| path.is_absolute())
}

fn absolute_or_default(
    override_path: Option<&Path>,
    home: &Path,
    fallback: &[&str],
) -> Result<PathBuf, PlatformError> {
    if let Some(path) = absolute_optional(override_path) {
        return Ok(path.to_path_buf());
    }
    join_absolute(home, "home", fallback)
}

/// `WIND_RUNTIME_ROOT` selects a clean absolute profile root.
///
/// An unset or empty value keeps the normal operating-system profile paths.
fn runtime_root_override(
    value: Option<std::ffi::OsString>,
) -> Result<Option<PathBuf>, PlatformError> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(PlatformError::RelativePath {
            role: "WIND_RUNTIME_ROOT",
            path,
        });
    }
    Ok(Some(path))
}

fn env_path(variable: &str) -> Option<PathBuf> {
    let value = std::env::var_os(variable)?;
    let path = PathBuf::from(value);
    if path.as_os_str().is_empty() || !path.is_absolute() {
        None
    } else {
        Some(path)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn required_env_path(variable: &'static str) -> Result<PathBuf, PlatformError> {
    env_path(variable).ok_or(PlatformError::MissingLocation { variable })
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_runtime_paths, host_runtime_paths, linux_runtime_paths, macos_runtime_paths,
        windows_runtime_paths, LinuxPathEnv,
    };
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn anchor() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("wind-paths-{}-{nanos}", std::process::id()))
    }

    fn contains_component(path: &Path, name: &str) -> bool {
        path.components()
            .any(|component| component.as_os_str() == name)
    }

    #[test]
    fn platform_paths_are_absolute_deterministic_and_identifier_based() {
        let home = anchor();
        let identifier = "com.example.agent";
        let macos = macos_runtime_paths(&home, identifier).unwrap();
        assert_eq!(macos, macos_runtime_paths(&home, identifier).unwrap());
        assert!(macos.config_dir.ends_with(
            Path::new("Library")
                .join("Application Support")
                .join(identifier)
                .join("config")
        ));
        assert!(macos
            .cache_dir
            .ends_with(Path::new("Library").join("Caches").join(identifier)));
        assert!(macos
            .log_dir
            .ends_with(Path::new("Library").join("Logs").join(identifier)));
        assert!(macos.runtime_dir.ends_with("runtime"));

        let windows = windows_runtime_paths(&home, &home, identifier).unwrap();
        assert_eq!(
            windows,
            windows_runtime_paths(&home, &home, identifier).unwrap()
        );
        assert!(windows
            .config_dir
            .ends_with(Path::new(identifier).join("config")));
        assert!(windows
            .data_dir
            .ends_with(Path::new(identifier).join("data")));
        assert!(windows
            .cache_dir
            .ends_with(Path::new(identifier).join("cache")));
        assert!(windows
            .log_dir
            .ends_with(Path::new(identifier).join("logs")));
        assert!(windows
            .runtime_dir
            .ends_with(Path::new(identifier).join("runtime")));

        let linux = linux_runtime_paths(
            &LinuxPathEnv {
                home: &home,
                config_home: None,
                data_home: None,
                cache_home: None,
                state_home: None,
                runtime_dir: None,
            },
            identifier,
        )
        .unwrap();
        assert!(linux
            .config_dir
            .ends_with(Path::new(".config").join(identifier)));
        assert!(linux
            .data_dir
            .ends_with(Path::new(".local").join("share").join(identifier)));
        assert!(linux
            .cache_dir
            .ends_with(Path::new(".cache").join(identifier)));
        assert!(linux.log_dir.ends_with(
            Path::new(".local")
                .join("state")
                .join(identifier)
                .join("log")
        ));
        assert!(linux.runtime_dir.ends_with(
            Path::new(".local")
                .join("state")
                .join(identifier)
                .join("runtime")
        ));

        for paths in [&macos, &windows, &linux] {
            for (role, path) in paths.directories() {
                assert!(path.is_absolute(), "{role} {}", path.display());
                assert!(contains_component(path, identifier), "{role}");
                assert!(!contains_component(path, "Wind"), "{role}");
                assert_ne!(path, Path::new("config"));
                assert_ne!(path, Path::new("logs"));
                assert_ne!(path, Path::new("data"));
            }
        }
    }

    #[test]
    fn linux_xdg_overrides_are_absolute_and_relative_values_fall_back() {
        let home = anchor();
        let config_home = home.join("xdg-config");
        let runtime = home.join("xdg-run");
        let relative = Path::new("relative");
        let paths = linux_runtime_paths(
            &LinuxPathEnv {
                home: &home,
                config_home: Some(&config_home),
                data_home: Some(relative),
                cache_home: None,
                state_home: None,
                runtime_dir: Some(&runtime),
            },
            "com.example.agent",
        )
        .unwrap();
        assert_eq!(paths.config_dir, config_home.join("com.example.agent"));
        assert!(paths
            .data_dir
            .ends_with(Path::new(".local").join("share").join("com.example.agent")));
        assert_eq!(paths.runtime_dir, runtime.join("com.example.agent"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_override_root_replaces_the_home_anchor() {
        let home = anchor();
        let root = anchor();
        let paths = super::macos_paths(&home, Some(&root), "com.wind.agent").unwrap();
        assert!(paths.data_dir.starts_with(&root));
        assert!(!paths.data_dir.starts_with(&home));
        assert!(paths.data_dir.ends_with(
            Path::new("Library")
                .join("Application Support")
                .join("com.wind.agent")
                .join("data")
        ));
        assert!(paths.log_dir.starts_with(&root));
    }

    #[test]
    fn path_identifiers_reject_separators() {
        let home = anchor();
        assert!(macos_runtime_paths(&home, "../etc").is_err());
        assert!(macos_runtime_paths(&home, "a/b").is_err());
        assert!(windows_runtime_paths(&home, &home, r"a\b").is_err());
        assert!(macos_runtime_paths(Path::new("relative"), "com.example.agent").is_err());
    }

    #[test]
    fn ensure_creates_user_scoped_directories_without_protected_locations() {
        let home = anchor();
        let paths = windows_runtime_paths(&home, &home, "com.example.agent").unwrap();
        ensure_runtime_paths(&paths).unwrap();
        for (_, path) in paths.directories() {
            assert!(path.is_dir(), "{}", path.display());
            assert!(path.starts_with(&home));
        }
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn runtime_root_override_accepts_only_an_absolute_path() {
        assert!(super::runtime_root_override(None).unwrap().is_none());
        assert!(
            super::runtime_root_override(Some(std::ffi::OsString::new()))
                .unwrap()
                .is_none()
        );
        assert!(super::runtime_root_override(Some("relative".into())).is_err());
        let absolute = if cfg!(windows) {
            std::ffi::OsString::from(r"D:\wind-phase10")
        } else {
            std::ffi::OsString::from("/tmp/wind-phase11")
        };
        let root = super::runtime_root_override(Some(absolute))
            .unwrap()
            .unwrap();
        assert!(root.is_absolute());
    }

    #[test]
    fn host_runtime_paths_are_absolute_and_ignore_relative_layout() {
        let paths = host_runtime_paths("com.example.agent").expect("host home");
        for (role, path) in paths.directories() {
            assert!(path.is_absolute(), "{role} {}", path.display());
            assert!(contains_component(path, "com.example.agent"), "{role}");
            assert_ne!(path, Path::new("config"));
            assert_ne!(path, Path::new("logs"));
            assert_ne!(path, Path::new("data"));
        }
    }
}
