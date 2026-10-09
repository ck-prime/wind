use std::fmt;

/// Independent `major.minor.patch` version.
///
/// Product versions and service versions both use this type. Each owner stores
/// its own value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    pub fn parse(input: &str) -> std::result::Result<Self, VersionError> {
        let mut parts = input.split('.');
        let major = parse_component(input, parts.next())?;
        let minor = parse_component(input, parts.next())?;
        let patch = parse_component(input, parts.next())?;
        if parts.next().is_some() {
            return Err(VersionError {
                input: input.to_string(),
            });
        }
        Ok(Self::new(major, minor, patch))
    }
}

fn parse_component(input: &str, part: Option<&str>) -> std::result::Result<u64, VersionError> {
    let invalid = || VersionError {
        input: input.to_string(),
    };
    let Some(part) = part else {
        return Err(invalid());
    };
    if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    if part.len() > 1 && part.starts_with('0') {
        return Err(invalid());
    }
    part.parse().map_err(|_| invalid())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionError {
    input: String,
}

impl fmt::Display for VersionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid version '{}': expected major.minor.patch",
            self.input
        )
    }
}

impl std::error::Error for VersionError {}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::Version;

    #[test]
    fn parses_release_versions() {
        assert_eq!(Version::parse("0.1.0").unwrap(), Version::new(0, 1, 0));
        assert_eq!(Version::parse("2.3.0").unwrap().to_string(), "2.3.0");
    }

    #[test]
    fn rejects_incomplete_or_decorated_versions() {
        for input in ["1", "1.2", "1.2.3.4", "01.2.3", "v1.2.3", "1.2.x", ""] {
            assert!(Version::parse(input).is_err(), "{input}");
        }
    }
}
