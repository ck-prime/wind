//! HTTP and HTTPS URL sanitization.
//!
//! Credentials and a conservative set of query names are removed. This is not
//! a complete secret detector.

use url::Url;

use crate::contract::UrlError;

const SENSITIVE_QUERY: &[&str] = &[
    "access_token",
    "api_key",
    "apikey",
    "auth",
    "authorization",
    "code",
    "credential",
    "credentials",
    "id_token",
    "jwt",
    "key",
    "otp",
    "passwd",
    "password",
    "pwd",
    "refresh_token",
    "secret",
    "session",
    "session_id",
    "sessionid",
    "sig",
    "signature",
    "token",
];

/// Sanitize an address-field value.
///
/// A value that already has a scheme uses [`sanitize_http_url`]. A schemeless
/// host, which is how some browsers expose the address field, is read as
/// `https` and then sanitized. Search text is rejected.
pub fn sanitize_address_bar(raw: &str) -> Result<String, UrlError> {
    match sanitize_http_url(raw) {
        Ok(url) => Ok(url),
        Err(error) => {
            let trimmed = raw.trim();
            if !schemeless_host(trimmed) {
                return Err(error);
            }
            sanitize_http_url(&format!("https://{trimmed}"))
        }
    }
}

fn schemeless_host(value: &str) -> bool {
    if value.is_empty() || value.contains(' ') || value.contains("://") {
        return false;
    }
    let host = value.split(['/', '?', '#']).next().unwrap_or(value);
    let host = host.split('@').next_back().unwrap_or(host);
    host.eq_ignore_ascii_case("localhost") || host.contains('.')
}

/// Keep an `http` or `https` URL without userinfo, fragments, or listed query names.
pub fn sanitize_http_url(raw: &str) -> Result<String, UrlError> {
    if raw.len() > 4096 {
        return Err(UrlError::InvalidUrl {
            reason: "url is longer than 4096 bytes".to_string(),
        });
    }
    let mut parsed = Url::parse(raw).map_err(|_| UrlError::InvalidUrl {
        reason: "url is not a valid URI".to_string(),
    })?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(UrlError::InvalidUrl {
            reason: "only http and https urls are accepted".to_string(),
        });
    }
    if parsed.host_str().is_none() {
        return Err(UrlError::InvalidUrl {
            reason: "url has no host".to_string(),
        });
    }
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    parsed.set_fragment(None);
    let had_query = parsed.query().is_some();
    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(name, _)| !sensitive(name))
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    if !had_query {
        return Ok(parsed.to_string());
    }
    if kept.is_empty() {
        parsed.set_query(None);
    } else {
        parsed.query_pairs_mut().clear().extend_pairs(kept);
    }
    Ok(parsed.to_string())
}

fn sensitive(name: &str) -> bool {
    let folded = name.to_ascii_lowercase();
    SENSITIVE_QUERY.contains(&folded.as_str())
}

#[cfg(test)]
mod tests {
    use super::sanitize_http_url;
    use crate::UrlError;

    #[test]
    fn https_path_and_ordinary_query_are_kept() {
        assert_eq!(
            sanitize_http_url("https://example.com/path?a=1").unwrap(),
            "https://example.com/path?a=1"
        );
        assert_eq!(
            sanitize_http_url("http://example.com").unwrap(),
            "http://example.com/"
        );
        assert_eq!(
            sanitize_http_url("https://example.com/path").unwrap(),
            "https://example.com/path"
        );
    }

    #[test]
    fn a_sensitive_query_and_fragment_are_removed_without_echoing_them() {
        let sanitized =
            sanitize_http_url("https://example.com/path?token=test-secret&a=1#fragment").unwrap();
        assert_eq!(sanitized, "https://example.com/path?a=1");
        let rejected = sanitize_http_url("https://user:password@").unwrap_err();
        let rendered = rejected.to_string();
        assert!(!rendered.contains("password"));
        assert!(!rendered.contains("test-secret"));
    }

    #[test]
    fn a_schemeless_address_bar_host_is_read_as_https() {
        assert_eq!(
            super::sanitize_address_bar("example.com").unwrap(),
            "https://example.com/"
        );
        assert_eq!(
            super::sanitize_address_bar("example.com/path?a=1").unwrap(),
            "https://example.com/path?a=1"
        );
        assert!(super::sanitize_address_bar("kittens").is_err());
    }

    #[test]
    fn credentials_fragment_and_token_parameters_are_removed() {
        let sanitized =
            sanitize_http_url("https://user:password@example.com/reset?token=abc&a=1#secret")
                .unwrap();
        assert_eq!(sanitized, "https://example.com/reset?a=1");
        assert!(!sanitized.contains("user"));
        assert!(!sanitized.contains("password"));
        assert!(!sanitized.contains("token"));
        assert!(!sanitized.contains("secret"));
    }

    #[test]
    fn other_schemes_and_malformed_values_are_rejected() {
        for raw in [
            "file:///tmp/secret",
            "javascript:alert(1)",
            "data:text/plain,hi",
            "chrome://settings",
            "about:blank",
            "ftp://example.com",
            "not a url",
            "invalid-url",
            "",
        ] {
            assert!(
                matches!(sanitize_http_url(raw), Err(UrlError::InvalidUrl { .. })),
                "{raw}"
            );
        }
    }
}
