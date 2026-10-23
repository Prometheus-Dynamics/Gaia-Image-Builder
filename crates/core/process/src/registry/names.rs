//! Checks and helpers for registry names, tags, digests and hosts.

use super::*;

/// Percent-encodes everything but the RFC 3986 unreserved characters.
pub(super) fn url_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Repository names are checked before they reach a URL path.
pub(super) fn check_name(value: &str) -> Result<(), RegistryError> {
    let valid = value.len() <= 255
        && !value.starts_with('/')
        && !value.ends_with('/')
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'));
    if valid {
        Ok(())
    } else {
        Err(RegistryError::Protocol(format!(
            "'{value}' is not a registry name"
        )))
    }
}

/// Tags follow the OCI grammar: at most 128 characters of `[A-Za-z0-9_.-]`.
pub(super) fn check_tag(value: &str) -> Result<(), RegistryError> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && !value.starts_with(['.', '-'])
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if valid {
        Ok(())
    } else {
        Err(RegistryError::Protocol(format!(
            "'{value}' is not a registry tag"
        )))
    }
}

/// A digest is `sha256:` and 64 lowercase hex digits.
pub(super) fn check_digest(value: &str) -> Result<(), RegistryError> {
    let hex = value.strip_prefix("sha256:").unwrap_or("");
    if hex.len() == 64
        && hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(RegistryError::Protocol(
            "a registry digest is not sha256:<hex>".into(),
        ))
    }
}

pub(super) fn check_reference(value: &str) -> Result<(), RegistryError> {
    if value.starts_with("sha256:") {
        check_digest(value)
    } else {
        check_tag(value)
    }
}

/// The host (without port or path) of a realm URL, such as
/// `https://ghcr.io/token` gives `ghcr.io`. `None` for a URL without a host.
pub(super) fn realm_host(realm: &str) -> Option<&str> {
    let rest = realm.split_once("://").map_or(realm, |(_, rest)| rest);
    let host = rest.split(['/', '?']).next()?;
    (!host.is_empty()).then(|| host_of(host))
}

/// The host of a `host[:port]` string, without the port.
pub(super) fn host_of(authority: &str) -> &str {
    if let Some(bracketed) = authority.strip_prefix('[') {
        return bracketed.split(']').next().unwrap_or(authority);
    }
    authority.split(':').next().unwrap_or(authority)
}

#[cfg(test)]
mod realm_host_tests {
    use super::*;

    #[test]
    fn the_realm_host_is_compared_without_port_or_path() {
        assert_eq!(realm_host("https://ghcr.io/token"), Some("ghcr.io"));
        assert_eq!(realm_host("http://127.0.0.1:5000/token"), Some("127.0.0.1"));
        assert_eq!(
            realm_host("https://auth.example.com:443"),
            Some("auth.example.com")
        );
        assert_eq!(host_of("ghcr.io"), "ghcr.io");
        assert_eq!(host_of("[::1]:5000"), "::1");
        assert_eq!(realm_host("https://"), None);
    }
}
