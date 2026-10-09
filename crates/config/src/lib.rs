//! Loader for `config/product.toml`.
//!
//! This is the only crate that parses product configuration.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use wind_core::{technical_id, ServiceId, Version, SUPERVISOR_COMPONENT};

#[cfg(any(test, not(debug_assertions)))]
const EMBEDDED_PRODUCT_TOML: &str = include_str!("../../../config/product.toml");
#[cfg(any(test, not(debug_assertions)))]
const EMBEDDED_LOGO_PATH: &str = "assets/branding/logo.svg";

#[cfg(debug_assertions)]
const ENV_PRODUCT_CONFIG: &str = "WIND_PRODUCT_CONFIG";
#[cfg(debug_assertions)]
const FILE_NAME: &str = "product.toml";

/// Used when a service has no `[services.<id>]` table.
pub const DEFAULT_SERVICE_ENABLED: bool = true;

/// Used when `[runtime] health_check_interval_seconds` is omitted.
pub const DEFAULT_HEALTH_CHECK_INTERVAL_SECONDS: u64 = 10;

/// Used when `[runtime] shutdown_timeout_seconds` is omitted.
///
/// This is the same budget `wind-service-manager` uses when a caller does not
/// load product configuration.
pub const DEFAULT_SHUTDOWN_TIMEOUT_SECONDS: u64 = 5;

/// Validated product branding and technical identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductConfig {
    product: ProductIdentity,
    branding: BrandingConfig,
    company: CompanyConfig,
    services: ServicesConfig,
    runtime: RuntimeConfig,
    source_path: Option<PathBuf>,
}

/// Resident-runtime settings. Missing `[runtime]` fields use the defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    health_check_interval: Duration,
    shutdown_timeout: Duration,
}

/// Per-service enable flags. Missing tables use [`DEFAULT_SERVICE_ENABLED`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicesConfig {
    screenshot: bool,
    activity: bool,
    app_tracker: bool,
    url_tracker: bool,
    video: bool,
}

/// Technical product identity and the visible product name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductIdentity {
    name: String,
    display_name: String,
    identifier: String,
    version: Version,
}

/// Branding assets. Paths stay relative until resolved against the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrandingConfig {
    logo: PathBuf,
}

/// Company identity used by future installer and about surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanyConfig {
    name: String,
}

#[derive(Debug)]
pub enum ConfigError {
    NotFound {
        reason: String,
    },
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Parse {
        path: Option<PathBuf>,
        source: Box<toml::de::Error>,
    },
    Invalid {
        path: Option<PathBuf>,
        field: &'static str,
        reason: String,
    },
}

impl ProductConfig {
    pub fn product(&self) -> &ProductIdentity {
        &self.product
    }

    pub fn branding(&self) -> &BrandingConfig {
        &self.branding
    }

    pub fn company(&self) -> &CompanyConfig {
        &self.company
    }

    pub fn services(&self) -> &ServicesConfig {
        &self.services
    }

    pub fn runtime(&self) -> &RuntimeConfig {
        &self.runtime
    }

    pub fn source_path(&self) -> Option<&Path> {
        self.source_path.as_deref()
    }

    /// Resolve `branding.logo` against the parent of the `config` directory.
    pub fn resolved_logo_path(&self) -> Result<PathBuf, ConfigError> {
        let Some(source) = &self.source_path else {
            return Err(invalid(
                None,
                "branding.logo",
                "cannot resolve a relative logo path without a configuration file location",
            ));
        };
        let config_dir = source.parent().ok_or_else(|| {
            invalid(
                Some(source),
                "branding.logo",
                "configuration path has no parent directory",
            )
        })?;
        let root = config_dir.parent().ok_or_else(|| {
            invalid(
                Some(source),
                "branding.logo",
                "configuration directory has no parent directory",
            )
        })?;
        Ok(root.join(&self.branding.logo))
    }
}

impl ProductIdentity {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn identifier(&self) -> &str {
        &self.identifier
    }

    pub fn version(&self) -> Version {
        self.version
    }

    pub fn supervisor_technical_id(&self) -> String {
        technical_id(&self.identifier, SUPERVISOR_COMPONENT)
    }

    pub fn service_technical_id(&self, service: ServiceId) -> String {
        technical_id(&self.identifier, service.technical_component())
    }
}

impl BrandingConfig {
    pub fn logo(&self) -> &Path {
        &self.logo
    }
}

impl CompanyConfig {
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl RuntimeConfig {
    pub fn health_check_interval(&self) -> Duration {
        self.health_check_interval
    }

    pub fn shutdown_timeout(&self) -> Duration {
        self.shutdown_timeout
    }
}

impl ServicesConfig {
    pub fn enabled(&self, id: ServiceId) -> bool {
        match id {
            ServiceId::Screenshot => self.screenshot,
            ServiceId::Activity => self.activity,
            ServiceId::AppTracker => self.app_tracker,
            ServiceId::UrlTracker => self.url_tracker,
            ServiceId::Video => self.video,
        }
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { reason } => write!(formatter, "{reason}"),
            Self::Io { path, source } => {
                write!(formatter, "failed to read {}: {source}", path.display())
            }
            Self::Parse { path, source } => match path {
                Some(path) => write!(formatter, "failed to parse {}: {source}", path.display()),
                None => write!(formatter, "failed to parse product configuration: {source}"),
            },
            Self::Invalid {
                path,
                field,
                reason,
            } => match path {
                Some(path) => write!(
                    formatter,
                    "invalid configuration at {}: {field} {reason}",
                    path.display()
                ),
                None => write!(formatter, "invalid product configuration: {field} {reason}"),
            },
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source.as_ref()),
            Self::NotFound { .. } | Self::Invalid { .. } => None,
        }
    }
}

/// Load product configuration.
///
/// Development builds read `config/product.toml` from the usual lookup
/// locations. Release builds parse the configuration and logo embedded from
/// that same file and do not read either resource from disk.
pub fn load_product_config() -> Result<ProductConfig, ConfigError> {
    #[cfg(debug_assertions)]
    {
        load_product_config_from(&locate_product_config()?)
    }
    #[cfg(not(debug_assertions))]
    {
        load_embedded_product_config()
    }
}

#[cfg(any(test, not(debug_assertions)))]
fn load_embedded_product_config() -> Result<ProductConfig, ConfigError> {
    let config = parse_product_config(EMBEDDED_PRODUCT_TOML)?;
    if config.branding().logo() != Path::new(EMBEDDED_LOGO_PATH) {
        return Err(invalid(
            None,
            "branding.logo",
            format!("embedded branding resource is {EMBEDDED_LOGO_PATH}"),
        ));
    }
    if embedded_logo_bytes().is_empty() {
        return Err(invalid(None, "branding.logo", "embedded logo is empty"));
    }
    Ok(config)
}

#[cfg(any(test, not(debug_assertions)))]
#[used]
static EMBEDDED_LOGO: &[u8] = include_bytes!("../../../assets/branding/logo.svg");

#[cfg(any(test, not(debug_assertions)))]
fn embedded_logo_bytes() -> &'static [u8] {
    std::hint::black_box(EMBEDDED_LOGO)
}

/// Load and validate one configuration file, including its logo path.
pub fn load_product_config_from(path: &Path) -> Result<ProductConfig, ConfigError> {
    let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let config = parse_product_config_at(&text, Some(path))?;
    let logo = config.resolved_logo_path()?;
    if !logo.is_file() {
        return Err(invalid(
            Some(path),
            "branding.logo",
            format!("logo file does not exist at {}", logo.display()),
        ));
    }
    Ok(config)
}

/// Parse configuration text without touching the filesystem.
pub fn parse_product_config(toml_text: &str) -> Result<ProductConfig, ConfigError> {
    parse_product_config_at(toml_text, None)
}

fn parse_product_config_at(
    toml_text: &str,
    path: Option<&Path>,
) -> Result<ProductConfig, ConfigError> {
    let raw: RawConfig = toml::from_str(toml_text).map_err(|source| ConfigError::Parse {
        path: path.map(Path::to_path_buf),
        source: Box::new(source),
    })?;
    validate(&raw, path)?;
    let version = Version::parse(&raw.product.version)
        .map_err(|error| invalid(path, "product.version", error.to_string()))?;
    let runtime = RuntimeConfig {
        health_check_interval: runtime_duration(
            path,
            "runtime.health_check_interval_seconds",
            raw.runtime.health_check_interval_seconds,
            DEFAULT_HEALTH_CHECK_INTERVAL_SECONDS,
        )?,
        shutdown_timeout: runtime_duration(
            path,
            "runtime.shutdown_timeout_seconds",
            raw.runtime.shutdown_timeout_seconds,
            DEFAULT_SHUTDOWN_TIMEOUT_SECONDS,
        )?,
    };
    Ok(ProductConfig {
        product: ProductIdentity {
            name: raw.product.name,
            display_name: raw.product.display_name,
            identifier: raw.product.identifier,
            version,
        },
        branding: BrandingConfig {
            logo: PathBuf::from(raw.branding.logo),
        },
        company: CompanyConfig {
            name: raw.company.name,
        },
        services: ServicesConfig {
            screenshot: toggle_enabled(raw.services.screenshot),
            activity: toggle_enabled(raw.services.activity),
            app_tracker: toggle_enabled(raw.services.app_tracker),
            url_tracker: toggle_enabled(raw.services.url_tracker),
            video: toggle_enabled(raw.services.video),
        },
        runtime,
        source_path: path.map(Path::to_path_buf),
    })
}

fn runtime_duration(
    path: Option<&Path>,
    field: &'static str,
    value: Option<u64>,
    default_seconds: u64,
) -> Result<Duration, ConfigError> {
    match value {
        None => Ok(Duration::from_secs(default_seconds)),
        Some(0) => Err(invalid(path, field, "must be greater than zero")),
        Some(seconds) => Ok(Duration::from_secs(seconds)),
    }
}

fn toggle_enabled(toggle: Option<RawServiceToggle>) -> bool {
    toggle
        .map(|value| value.enabled)
        .unwrap_or(DEFAULT_SERVICE_ENABLED)
}

fn validate(raw: &RawConfig, path: Option<&Path>) -> Result<(), ConfigError> {
    require_text(path, "product.name", &raw.product.name)?;
    require_text(path, "product.display_name", &raw.product.display_name)?;
    require_text(path, "product.identifier", &raw.product.identifier)?;
    validate_identifier(path, &raw.product.identifier)?;
    require_text(path, "product.version", &raw.product.version)?;
    require_text(path, "branding.logo", &raw.branding.logo)?;
    validate_logo(path, &raw.branding.logo)?;
    require_text(path, "company.name", &raw.company.name)?;
    Ok(())
}

fn require_text(path: Option<&Path>, field: &'static str, value: &str) -> Result<(), ConfigError> {
    if value.trim().is_empty() {
        return Err(invalid(path, field, "must not be empty"));
    }
    if value != value.trim() {
        return Err(invalid(
            path,
            field,
            "must not have leading or trailing whitespace",
        ));
    }
    Ok(())
}

fn validate_identifier(path: Option<&Path>, value: &str) -> Result<(), ConfigError> {
    let bytes = value.as_bytes();
    if bytes.first() == Some(&b'.') || bytes.last() == Some(&b'.') {
        return Err(invalid(
            path,
            "product.identifier",
            "must not start or end with '.'",
        ));
    }
    if !value.contains('.') {
        return Err(invalid(
            path,
            "product.identifier",
            "must be a dotted identifier",
        ));
    }
    if value.contains("..") {
        return Err(invalid(path, "product.identifier", "must not contain '..'"));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
    {
        return Err(invalid(
            path,
            "product.identifier",
            "must contain only ASCII letters, digits, '.' and '-'",
        ));
    }
    Ok(())
}

fn validate_logo(path: Option<&Path>, logo: &str) -> Result<(), ConfigError> {
    let logo_path = Path::new(logo);
    if logo_path.is_absolute() {
        return Err(invalid(
            path,
            "branding.logo",
            "must be a path relative to the workspace root",
        ));
    }
    if logo_path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(invalid(path, "branding.logo", "must not contain '..'"));
    }
    Ok(())
}

fn invalid(path: Option<&Path>, field: &'static str, reason: impl Into<String>) -> ConfigError {
    ConfigError::Invalid {
        path: path.map(Path::to_path_buf),
        field,
        reason: reason.into(),
    }
}

#[cfg(debug_assertions)]
fn locate_product_config() -> Result<PathBuf, ConfigError> {
    if let Some(configured) = std::env::var_os(ENV_PRODUCT_CONFIG) {
        let path = PathBuf::from(configured);
        if path.is_file() {
            return Ok(path);
        }
        return Err(ConfigError::NotFound {
            reason: format!(
                "{ENV_PRODUCT_CONFIG} points to '{}', which is not a file",
                path.display()
            ),
        });
    }

    let mut starts = Vec::new();
    if let Ok(current) = std::env::current_dir() {
        starts.push(current);
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            starts.push(directory.to_path_buf());
        }
    }
    starts.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));

    for start in &starts {
        if let Some(found) = walk_for_product_config(start) {
            return Ok(found);
        }
    }

    Err(ConfigError::NotFound {
        reason: format!(
            "config/{FILE_NAME} was not found from the working directory or the executable directory. Set {ENV_PRODUCT_CONFIG} to its path."
        ),
    })
}

#[cfg(debug_assertions)]
fn walk_for_product_config(start: &Path) -> Option<PathBuf> {
    let mut directory = Some(start);
    for _ in 0..8 {
        let current = directory?;
        let candidate = current.join("config").join(FILE_NAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        directory = current.parent();
    }
    None
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    product: RawProduct,
    branding: RawBranding,
    company: RawCompany,
    #[serde(default)]
    services: RawServices,
    #[serde(default)]
    runtime: RawRuntime,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProduct {
    name: String,
    display_name: String,
    identifier: String,
    version: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBranding {
    logo: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCompany {
    name: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct RawServices {
    screenshot: Option<RawServiceToggle>,
    activity: Option<RawServiceToggle>,
    app_tracker: Option<RawServiceToggle>,
    url_tracker: Option<RawServiceToggle>,
    video: Option<RawServiceToggle>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawServiceToggle {
    enabled: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct RawRuntime {
    health_check_interval_seconds: Option<u64>,
    shutdown_timeout_seconds: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::{
        load_embedded_product_config, load_product_config, load_product_config_from,
        parse_product_config, DEFAULT_HEALTH_CHECK_INTERVAL_SECONDS,
        DEFAULT_SHUTDOWN_TIMEOUT_SECONDS,
    };
    use std::path::Path;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use wind_core::ServiceId;

    const VALID: &str = r#"
[product]
name = "Fixture"
display_name = "Fixture Product"
identifier = "com.example.agent"
version = "0.1.0"

[branding]
logo = "assets/branding/logo.svg"

[company]
name = "Fixture Co"
"#;

    #[test]
    fn valid_product_configuration_exposes_name_and_identifier() {
        let config = parse_product_config(VALID).unwrap();
        assert_eq!(config.product().name(), "Fixture");
        assert_eq!(config.product().display_name(), "Fixture Product");
        assert_eq!(config.product().identifier(), "com.example.agent");
        assert_eq!(config.product().version().to_string(), "0.1.0");
        assert_eq!(config.company().name(), "Fixture Co");
        assert_eq!(
            config.branding().logo(),
            Path::new("assets/branding/logo.svg")
        );
        assert_eq!(
            config.product().supervisor_technical_id(),
            "com.example.agent.supervisor"
        );
        assert!(ServiceId::ALL
            .iter()
            .all(|id| config.services().enabled(*id)));
        assert_eq!(
            config.runtime().health_check_interval(),
            Duration::from_secs(DEFAULT_HEALTH_CHECK_INTERVAL_SECONDS)
        );
        assert_eq!(
            config.runtime().shutdown_timeout(),
            Duration::from_secs(DEFAULT_SHUTDOWN_TIMEOUT_SECONDS)
        );
    }

    #[test]
    fn runtime_intervals_come_from_configuration() {
        let text = format!(
            "{VALID}\n[runtime]\nhealth_check_interval_seconds = 2\nshutdown_timeout_seconds = 3\n"
        );
        let config = parse_product_config(&text).unwrap();
        assert_eq!(
            config.runtime().health_check_interval(),
            Duration::from_secs(2)
        );
        assert_eq!(config.runtime().shutdown_timeout(), Duration::from_secs(3));

        let zero = parse_product_config(&format!(
            "{VALID}\n[runtime]\nhealth_check_interval_seconds = 0\n"
        ))
        .unwrap_err();
        let zero_text = zero.to_string();
        assert!(
            zero_text.contains("health_check_interval_seconds"),
            "{zero_text}"
        );
        assert!(zero_text.contains("greater than zero"), "{zero_text}");
    }

    #[test]
    fn missing_service_configuration_defaults_to_enabled() {
        let config = parse_product_config(VALID).unwrap();
        assert!(config.services().enabled(ServiceId::Video));
    }

    #[test]
    fn service_enable_flags_come_from_configuration() {
        let text = format!(
            "{VALID}\n[services.video]\nenabled = false\n[services.activity]\nenabled = true\n"
        );
        let config = parse_product_config(&text).unwrap();
        assert!(!config.services().enabled(ServiceId::Video));
        assert!(config.services().enabled(ServiceId::Activity));
        assert!(config.services().enabled(ServiceId::Screenshot));
    }

    #[test]
    fn invalid_service_configuration_fails_clearly() {
        let unknown = parse_product_config(&format!(
            "{VALID}\n[services.not_a_service]\nenabled = true\n"
        ))
        .unwrap_err();
        let unknown_text = unknown.to_string();
        assert!(
            unknown_text.contains("not_a_service") || unknown_text.contains("unknown"),
            "{unknown_text}"
        );

        let missing_flag =
            parse_product_config(&format!("{VALID}\n[services.video]\n")).unwrap_err();
        let missing_text = missing_flag.to_string();
        assert!(missing_text.contains("enabled"), "{missing_text}");

        let wrong_type =
            parse_product_config(&format!("{VALID}\n[services.video]\nenabled = \"no\"\n"))
                .unwrap_err();
        let type_text = wrong_type.to_string();
        assert!(
            type_text.contains("enabled") || type_text.contains("video"),
            "{type_text}"
        );
    }

    #[test]
    fn invalid_configuration_fails_clearly() {
        let missing = parse_product_config("[company]\nname = \"Only\"\n").unwrap_err();
        let missing_text = missing.to_string();
        assert!(
            missing_text.contains("product") || missing_text.contains("missing"),
            "{missing_text}"
        );

        let empty_name = parse_product_config(
            &VALID.replace("display_name = \"Fixture Product\"", "display_name = \"\""),
        )
        .unwrap_err();
        let empty_text = empty_name.to_string();
        assert!(empty_text.contains("display_name"), "{empty_text}");
        assert!(empty_text.contains("must not be empty"), "{empty_text}");

        let bad_version =
            parse_product_config(&VALID.replace("version = \"0.1.0\"", "version = \"1.0\""))
                .unwrap_err();
        let version_text = bad_version.to_string();
        assert!(version_text.contains("product.version"), "{version_text}");
        assert!(version_text.contains("major.minor.patch"), "{version_text}");
    }

    #[test]
    fn shipped_product_configuration_loads() {
        let config = load_product_config().expect("workspace config/product.toml should load");
        assert_eq!(config.product().identifier(), "com.wind.agent");
        assert!(!config.product().display_name().is_empty());
        assert!(!config.product().name().is_empty());
        assert!(!config.company().name().is_empty());
        assert!(config.resolved_logo_path().unwrap().is_file());
        assert_eq!(
            config.product().supervisor_technical_id(),
            "com.wind.agent.supervisor"
        );
        assert!(config.services().enabled(ServiceId::Screenshot));
        assert!(!config.services().enabled(ServiceId::Video));
        assert_eq!(
            config.runtime().health_check_interval(),
            Duration::from_secs(10)
        );
        assert_eq!(config.runtime().shutdown_timeout(), Duration::from_secs(5));
    }

    #[test]
    fn missing_logo_file_fails_clearly() {
        let root = std::env::temp_dir().join(format!(
            "wind-config-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("config")).unwrap();
        let path = root.join("config").join("product.toml");
        std::fs::write(&path, VALID).unwrap();
        let error = load_product_config_from(&path).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("branding.logo"), "{text}");
        assert!(text.contains("does not exist"), "{text}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn embedded_product_configuration_matches_the_branding_source() {
        let config = load_embedded_product_config().expect("embedded product.toml");
        assert_eq!(config.product().identifier(), "com.wind.agent");
        assert_eq!(
            config.branding().logo(),
            Path::new("assets/branding/logo.svg")
        );
        assert!(!super::embedded_logo_bytes().is_empty());
        assert!(config.source_path().is_none());
    }
}
