//! Secrets in URLs and values are masked before a report shows them.

use gaia_report::{MASK, is_secret_name, mask_url, mask_value};

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
