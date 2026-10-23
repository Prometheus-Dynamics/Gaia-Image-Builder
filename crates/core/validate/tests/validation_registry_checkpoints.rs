pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use support::write_temp_config;

/// The diagnostic codes of a build config with the given checkpoint table.
fn codes(name: &str, checkpoint: &str) -> Vec<String> {
    let text = format!(
        r#"
build_name = "{name}"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

{checkpoint}
"#
    );
    let path = write_temp_config(&text);
    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let codes = validate_spec(&spec)
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect();
    let _ = fs::remove_file(path);
    codes
}

#[test]
fn pulls_without_a_verify_key_are_refused_by_validation() {
    let codes = codes(
        "oci-no-verify",
        r#"
[[checkpoints]]
id = "registry"
backend = "oci"
registry = "ghcr.io"
repository = "owner/gaia-cache"
use_policy = "auto"
upload_policy = "off"
"#,
    );
    assert!(
        codes
            .iter()
            .any(|code| code == "checkpoint_oci_verify_key_missing"),
        "{codes:?}"
    );
}

#[test]
fn trust_unsigned_allows_pulls_without_a_verify_key() {
    let codes = codes(
        "oci-trust-unsigned",
        r#"
[[checkpoints]]
id = "registry"
backend = "oci"
registry = "ghcr.io"
repository = "owner/gaia-cache"
use_policy = "auto"
upload_policy = "off"
trust = "unsigned"
"#,
    );
    assert!(
        !codes.iter().any(|code| code.starts_with("checkpoint_oci")),
        "{codes:?}"
    );
}

#[test]
fn a_complete_oci_checkpoint_is_accepted_and_carries_its_settings() {
    let path = write_temp_config(
        r#"
build_name = "oci-complete"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[[checkpoints]]
id = "registry"
backend = "oci"
registry = "ghcr.io"
repository = "owner/gaia-cache"
verify_key = "/keys/gaia.pub"
sign_key = "env:GAIA_TEST_SIGN_KEY_UNSET_ZZ"
sign_kind = "gpg"
use_policy = "auto"
upload_policy = "auto"
"#,
    );
    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);
    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert!(
        !codes
            .iter()
            .any(|code| code.starts_with("checkpoint_oci")
                && *code != "checkpoint_oci_sign_key_missing"),
        "{codes:?}"
    );
    let oci = spec.checkpoints.points[0]
        .backend
        .as_ref()
        .and_then(|backend| backend.oci.as_ref())
        .expect("oci settings");
    assert_eq!(oci.registry, "ghcr.io");
    assert_eq!(oci.repository, "owner/gaia-cache");
    assert_eq!(oci.verify_key.as_deref(), Some("/keys/gaia.pub"));
    assert_eq!(oci.sign_kind.as_deref(), Some("gpg"));
    assert!(!oci.trust_unsigned());
    let _ = fs::remove_file(path);
}

#[test]
fn registry_fields_on_another_backend_and_bad_values_are_rejected() {
    let other = codes(
        "oci-field-on-local",
        r#"
[[checkpoints]]
id = "local-with-registry"
backend = "local"
registry = "ghcr.io"
use_policy = "auto"
upload_policy = "off"
"#,
    );
    assert!(
        other
            .iter()
            .any(|code| code == "checkpoint_oci_field_without_backend"),
        "{other:?}"
    );

    let bad = codes(
        "oci-bad-values",
        r#"
[[checkpoints]]
id = "registry"
backend = "oci"
repository = "owner/gaia-cache"
verify_key = "/keys/gaia.pub"
sign_kind = "pgp"
trust = "maybe"
use_policy = "auto"
upload_policy = "off"
"#,
    );
    for expected in [
        "checkpoint_oci_registry_missing",
        "checkpoint_oci_sign_kind",
        "checkpoint_oci_trust",
    ] {
        assert!(
            bad.iter().any(|code| code == expected),
            "{expected}: {bad:?}"
        );
    }
}
