pub mod support;

use gaia_testkit::TempDir;
use support::{default_config_path, write_temp_config};

#[test]
fn rejects_deprecated_tuple_workspace_named_paths_shape() {
    let path = write_temp_config(
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
    );

    let result = gaia_config::try_resolve_config(path.to_str().expect("temp path should be utf-8"));

    let error = result.expect_err("deprecated workspace path shape should fail");
    assert!(matches!(
        error,
        gaia_config::ConfigError::ConfigShape { .. }
    ));
    assert!(error.to_string().contains("workspace.named_paths"));
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

#[test]
fn missing_import_names_the_importing_file() {
    let scratch = TempDir::new("gaia-missing-import");
    let dir = scratch.path();
    let build = dir.join("build.toml");
    std::fs::write(
        &build,
        "build_name = \"dead-ref\"\nimports = [\"layers/gone.toml\"]\n",
    )
    .expect("build config");

    let error = gaia_config::try_resolve_config(build.to_str().expect("utf-8 path"))
        .expect_err("missing import should fail");

    match &error {
        gaia_config::ConfigError::ConfigReferenceMissing {
            referenced_by,
            field,
            reference,
            ..
        } => {
            assert_eq!(*field, "import");
            assert_eq!(reference, "layers/gone.toml");
            assert!(referenced_by.ends_with("build.toml"));
        }
        other => panic!("expected missing reference error, got {other:?}"),
    }
    assert!(error.to_string().contains("import 'layers/gone.toml' in '"));
}
