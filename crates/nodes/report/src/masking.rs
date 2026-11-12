//! Secrets in what a report prints: URL user and password parts, and any
//! value whose name says it is a secret.

/// Name fragments (upper case) whose values are never printed.
const SECRET_NAMES: [&str; 8] = [
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "CREDENTIAL",
    "APIKEY",
    "API_KEY",
    "PRIVATE_KEY",
];

/// What a masked value is printed as.
pub const MASK: &str = "********";

/// Whether a name says its value is a secret.
pub fn is_secret_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_NAMES.iter().any(|part| upper.contains(part))
}

/// A `name = value` pair as printed: the value is masked when the name says
/// it is a secret.
pub fn mask_value(name: &str, value: &str) -> String {
    if is_secret_name(name) {
        MASK.to_string()
    } else {
        value.to_string()
    }
}

/// A URL as printed: no `user:password@`, and no query parameter whose name
/// says it is a secret.
pub fn mask_url(url: &str) -> String {
    let url = gaia_lock::redact(url);
    let Some((base, query)) = url.split_once('?') else {
        return url;
    };
    let query = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((name, value)) => format!("{name}={}", mask_value(name, value)),
            None => pair.to_string(),
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}?{query}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_userinfo_is_dropped() {
        assert_eq!(
            mask_url("https://user:hunter2@example.org/repo.git"),
            "https://example.org/repo.git"
        );
        assert_eq!(
            mask_url("https://token@example.org/repo.git"),
            "https://example.org/repo.git"
        );
        assert_eq!(
            mask_url("https://example.org/repo.git"),
            "https://example.org/repo.git"
        );
    }

    #[test]
    fn secret_query_values_are_masked() {
        assert_eq!(
            mask_url("https://example.org/dl?name=a&access_token=abc&x=1"),
            "https://example.org/dl?name=a&access_token=********&x=1"
        );
    }

    #[test]
    fn secret_names_mask_values() {
        assert_eq!(mask_value("API_KEY", "abc"), MASK);
        assert_eq!(mask_value("github-token", "abc"), MASK);
        assert_eq!(mask_value("cpu", "cortex-a55"), "cortex-a55");
        assert!(!is_secret_name("monkey"));
    }
}
