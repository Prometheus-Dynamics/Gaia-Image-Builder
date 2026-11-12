//! Library versions in keys: a key changes when a recorded version changes,
//! and Cargo.lock is what the check reads.

use gaia_step::{KeyBuilder, cargo_lock};

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

const LOCK: &str = r#"version = 4

[[package]]
name = "flate2"
version = "1.1.11"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = [
 "miniz_oxide",
]

[[package]]
name = "miniz_oxide"
version = "0.9.1"

[[package]]
name = "zstd-sys"
version = "2.1.2+zstd.1.5.7"

[[package]]
name = "zstd-sys"
version = "2.2.0+zstd.1.5.8"
"#;

#[test]
fn resolved_versions_reads_each_package_block() {
    assert_eq!(cargo_lock::resolved_versions(LOCK, "flate2"), ["1.1.11"]);
    assert_eq!(
        cargo_lock::resolved_versions(LOCK, "miniz_oxide"),
        ["0.9.1"]
    );
    assert_eq!(
        cargo_lock::resolved_versions(LOCK, "zstd-sys"),
        ["2.1.2+zstd.1.5.7", "2.2.0+zstd.1.5.8"]
    );
    assert!(cargo_lock::resolved_versions(LOCK, "xz2").is_empty());
}

#[test]
fn check_libs_reads_the_lock_beside_the_workspace_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let node = dir.path().join("crates/nodes/codec");
    std::fs::create_dir_all(&node).expect("node dir");
    std::fs::write(dir.path().join("Cargo.lock"), LOCK).expect("lock");
    let node = node.to_str().expect("utf-8");
    assert!(cargo_lock::check_libs(node, &[("flate2", "1.1.11")]).is_ok());
    let error = cargo_lock::check_libs(node, &[("flate2", "1.1.10"), ("xz2", "0.1.7")])
        .expect_err("a stale version and a missing crate both fail");
    assert!(error.contains("flate2: the step key names 1.1.10, Cargo.lock resolves 1.1.11"));
    assert!(error.contains("xz2: not in Cargo.lock"));
    let error = cargo_lock::check_libs(node, &[("zstd-sys", "2.1.2+zstd.1.5.7")])
        .expect_err("two versions are ambiguous");
    assert!(error.contains("zstd-sys: 2 versions"));
}
