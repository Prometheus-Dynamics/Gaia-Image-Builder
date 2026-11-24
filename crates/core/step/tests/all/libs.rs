//! Library versions in keys: a key changes when a recorded version changes,
//! and the pins a node's Cargo.toml holds are read by [`gaia_step::pins`].

use gaia_step::{KeyBuilder, pins};

#[test]
fn a_key_changes_when_a_library_version_changes() {
    let key = |version: &str| {
        KeyBuilder::new("gaia.test.codec", 1)
            .config("level", "6")
            .libs(&[("flate2", version), ("miniz_oxide", "0.9.1")])
            .finish()
    };
    assert_eq!(key("1.1.11"), key("1.1.11"));
    assert_ne!(key("1.1.11"), key("1.1.12"));
    let named = KeyBuilder::new("gaia.test.codec", 1)
        .config("level", "6")
        .lib("flate2", "1.1.11")
        .lib("miniz_oxide", "0.9.1");
    assert_eq!(named.finish(), key("1.1.11"));
    let without = KeyBuilder::new("gaia.test.codec", 1)
        .config("level", "6")
        .lib("flate2", "1.1.11")
        .finish();
    assert_ne!(
        without,
        key("1.1.11"),
        "a library missing from the key changes it"
    );
}

const MANIFEST: &str = r#"[dependencies]
flate2 = "=1.1.11"
miniz_oxide = { version = "=0.9.1", default-features = false }
tar = "0.4"
tar-extra = "=9.9.9"

[dev-dependencies]
flate2 = "1"
"#;

#[test]
fn pinned_versions_reads_only_exact_pins() {
    assert_eq!(pins::pinned_versions(MANIFEST, "flate2"), ["1.1.11"]);
    assert_eq!(pins::pinned_versions(MANIFEST, "miniz_oxide"), ["0.9.1"]);
    assert!(pins::pinned_versions(MANIFEST, "tar").is_empty());
    assert_eq!(pins::pinned_versions(MANIFEST, "tar-extra"), ["9.9.9"]);
    assert!(pins::pinned_versions(MANIFEST, "xz2").is_empty());
}

#[test]
fn check_pins_reads_the_node_manifest() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("Cargo.toml"), MANIFEST).expect("manifest");
    let node = dir.path().to_str().expect("utf-8");
    assert!(pins::check_pins(node, &[("flate2", "1.1.11")]).is_ok());
    let error = pins::check_pins(node, &[("flate2", "1.1.10"), ("tar", "0.4.46")])
        .expect_err("a stale pin and an unpinned crate both fail");
    assert!(error.contains("flate2: the step key names 1.1.10, Cargo.toml pins =1.1.11"));
    assert!(error.contains("tar: not pinned in Cargo.toml (=0.4.46)"));
}
