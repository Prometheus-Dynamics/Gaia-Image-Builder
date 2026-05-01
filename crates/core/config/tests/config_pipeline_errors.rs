pub mod support;

use std::time::{SystemTime, UNIX_EPOCH};
use support::default_config_path;

#[test]
fn rejects_deprecated_tuple_workspace_named_paths_shape() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("gaia-bad-workspace-paths-{nonce}.toml"));
    std::fs::write(
        &path,
        r#"
build_name = "bad-workspace-shape"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"
named_paths = [["assets", "assets"]]

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"
"#,
    )
    .expect("temp config should be written");

    let result = gaia_config::try_resolve_config(path.to_str().expect("temp path should be utf-8"));

    let error = result.expect_err("deprecated workspace path shape should fail");
    assert!(matches!(
        error,
        gaia_config::ConfigError::ConfigShape { .. }
    ));
    assert!(error.to_string().contains("workspace.named_paths"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn missing_build_config_returns_typed_error() {
    let result = gaia_config::try_resolve_config("definitely-not-a-gaia-build");

    assert!(matches!(
        result.expect_err("missing build should fail"),
        gaia_config::ConfigError::ConfigNotFound { .. }
    ));
}

#[test]
fn rejects_invalid_rollback_domain_override() {
    let result = gaia_config::try_resolve_config_with_options(
        &default_config_path(),
        &gaia_config::ResolveOptions {
            explicit_overrides: vec![(
                "policy.failure.rollback_domains".into(),
                "artifacts,not-real".into(),
            )],
            ..gaia_config::ResolveOptions::default()
        },
    );

    assert!(result.is_err());
    let error = result.expect_err("invalid rollback domain should fail");
    assert!(
        error
            .to_string()
            .contains("policy.failure.rollback_domains")
    );
    assert!(error.to_string().contains("not-real"));
}

#[test]
fn rejects_missing_preset_with_typed_error() {
    let result = gaia_config::try_resolve_config_with_options(
        &default_config_path(),
        &gaia_config::ResolveOptions {
            preset: Some("not-real".into()),
            ..gaia_config::ResolveOptions::default()
        },
    );

    assert_eq!(
        result.expect_err("missing preset should fail"),
        gaia_config::ConfigError::MissingPreset {
            preset: "not-real".into()
        }
    );
}

#[test]
fn rejects_invalid_enum_override_with_typed_error() {
    let result = gaia_config::try_resolve_config_with_options(
        &default_config_path(),
        &gaia_config::ResolveOptions {
            explicit_overrides: vec![("image.buildroot.external_tree_mode".into(), "maybe".into())],
            ..gaia_config::ResolveOptions::default()
        },
    );

    let error = result.expect_err("invalid enum override should fail");
    assert!(
        error
            .to_string()
            .contains("image.buildroot.external_tree_mode")
    );
    assert!(error.to_string().contains("maybe"));
}

#[test]
fn rejects_invalid_numeric_override_with_typed_error() {
    let result = gaia_config::try_resolve_config_with_options(
        &default_config_path(),
        &gaia_config::ResolveOptions {
            explicit_overrides: vec![(
                "policy.providers.buildroot.timeout_seconds".into(),
                "slow".into(),
            )],
            ..gaia_config::ResolveOptions::default()
        },
    );

    let error = result.expect_err("invalid numeric override should fail");
    assert!(
        error
            .to_string()
            .contains("policy.providers.buildroot.timeout_seconds")
    );
    assert!(error.to_string().contains("slow"));
}
