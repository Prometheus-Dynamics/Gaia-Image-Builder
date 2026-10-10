//! `export_dir` at both levels: `[image.output]` for one build and
//! `[workspace]` for the whole workspace. Covers the raw, override and
//! compile steps, relative and `~` paths, and empty values.

pub mod support;

use gaia_config::{ConfigError, ResolveOptions, try_resolve_config_with_options};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use support::write_temp_config;

/// A fresh absolute workspace root, so relative export paths have a known base.
fn workspace_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("gaia-export-config-{nonce}"))
}

/// A buildroot build rooted at `root`, with the given extra TOML appended to
/// the workspace section (`[workspace]` keys) and to the image output section.
fn build_toml(root: &Path, workspace_extra: &str, output_extra: &str) -> String {
    format!(
        r#"
build_name = "export-config"

[workspace]
root_dir = "{root}"
build_dir = "build"
out_dir = "out"
{workspace_extra}

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.output]
{output_extra}
"#,
        root = root.display(),
    )
}

fn resolve_with(
    toml: &str,
    explicit_overrides: Vec<(&str, &str)>,
) -> Result<gaia_spec::ResolvedBuildSpec, ConfigError> {
    let path = write_temp_config(toml);
    let result = try_resolve_config_with_options(
        path.to_str().expect("temp path should be utf-8"),
        &ResolveOptions {
            explicit_overrides: explicit_overrides
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
            ..ResolveOptions::default()
        },
    );
    let _ = std::fs::remove_file(path);
    result
}

fn resolve(toml: &str, explicit_overrides: Vec<(&str, &str)>) -> gaia_spec::ResolvedBuildSpec {
    resolve_with(toml, explicit_overrides).expect("config should resolve")
}

#[test]
fn no_export_setting_leaves_nothing_configured() {
    let root = workspace_root();
    let spec = resolve(&build_toml(&root, "", ""), vec![]);

    assert_eq!(spec.export.image_dir, None);
    assert_eq!(spec.export.workspace_dir, None);
    assert_eq!(spec.export.configured_dir(), None);
}

#[test]
fn image_export_dir_resolves_relative_to_the_workspace_root() {
    let root = workspace_root();
    let spec = resolve(
        &build_toml(&root, "", r#"export_dir = "exports/images""#),
        vec![],
    );

    let expected = root.join("exports/images").display().to_string();
    assert_eq!(spec.export.image_dir.as_deref(), Some(expected.as_str()));
    assert_eq!(spec.export.workspace_dir, None);
    assert_eq!(spec.export.configured_dir(), Some(expected.as_str()));
}

#[test]
fn workspace_export_dir_is_the_default_for_every_build() {
    let root = workspace_root();
    let spec = resolve(
        &build_toml(&root, r#"export_dir = "/srv/shared/images""#, ""),
        vec![],
    );

    assert_eq!(spec.export.image_dir, None);
    assert_eq!(
        spec.export.workspace_dir.as_deref(),
        Some("/srv/shared/images"),
        "an absolute workspace path is kept as written"
    );
    assert_eq!(spec.export.configured_dir(), Some("/srv/shared/images"));
}

#[test]
fn image_export_dir_takes_precedence_over_the_workspace_default() {
    let root = workspace_root();
    let spec = resolve(
        &build_toml(
            &root,
            r#"export_dir = "/srv/shared""#,
            r#"export_dir = "/srv/this-build""#,
        ),
        vec![],
    );

    assert_eq!(spec.export.image_dir.as_deref(), Some("/srv/this-build"));
    assert_eq!(spec.export.workspace_dir.as_deref(), Some("/srv/shared"));
    assert_eq!(spec.export.configured_dir(), Some("/srv/this-build"));
}

#[test]
fn set_overrides_reach_both_levels_and_follow_the_same_rules() {
    let root = workspace_root();
    let toml = build_toml(&root, "", "");

    let spec = resolve(
        &toml,
        vec![
            ("workspace.export_dir", "/srv/from-set"),
            ("image.output.export_dir", "built/images"),
        ],
    );
    let expected_image = root.join("built/images").display().to_string();
    assert_eq!(
        spec.export.image_dir.as_deref(),
        Some(expected_image.as_str())
    );
    assert_eq!(spec.export.workspace_dir.as_deref(), Some("/srv/from-set"));
    assert_eq!(spec.export.configured_dir(), Some(expected_image.as_str()));

    let workspace_only = resolve(&toml, vec![("workspace.export_dir", "/srv/from-set")]);
    assert_eq!(workspace_only.export.image_dir, None);
    assert_eq!(
        workspace_only.export.configured_dir(),
        Some("/srv/from-set")
    );
}

#[test]
fn a_set_override_beats_the_file_value() {
    let root = workspace_root();
    let spec = resolve(
        &build_toml(&root, "", r#"export_dir = "/srv/file""#),
        vec![("image.output.export_dir", "/srv/set")],
    );

    assert_eq!(spec.export.image_dir.as_deref(), Some("/srv/set"));
}

#[test]
fn tilde_expands_to_the_home_directory() {
    let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) else {
        eprintln!("HOME is not set; skipping the ~ expansion check");
        return;
    };
    let home = PathBuf::from(home);
    let root = workspace_root();

    let spec = resolve(
        &build_toml(
            &root,
            r#"export_dir = "~/gaia-exports""#,
            r#"export_dir = "~""#,
        ),
        vec![],
    );

    assert_eq!(
        spec.export.workspace_dir.as_deref(),
        Some(home.join("gaia-exports").display().to_string().as_str())
    );
    assert_eq!(
        spec.export.image_dir.as_deref(),
        Some(home.display().to_string().as_str())
    );
}

#[test]
fn only_a_leading_tilde_slash_expands() {
    let root = workspace_root();
    let spec = resolve(
        &build_toml(&root, "", r#"export_dir = "~user/images""#),
        vec![],
    );

    let expected = root.join("~user/images").display().to_string();
    assert_eq!(spec.export.image_dir.as_deref(), Some(expected.as_str()));
}

#[test]
fn an_empty_image_export_dir_is_an_error() {
    let root = workspace_root();
    let error = resolve_with(&build_toml(&root, "", r#"export_dir = "   ""#), vec![])
        .expect_err("an empty export_dir should fail");

    assert!(
        matches!(error, ConfigError::ConfigShape { .. }),
        "{error:?}"
    );
    assert!(
        error
            .to_string()
            .contains("image.output.export_dir cannot be empty"),
        "{error}"
    );
}

#[test]
fn an_empty_workspace_export_dir_is_an_error() {
    let root = workspace_root();
    let error = resolve_with(&build_toml(&root, r#"export_dir = """#, ""), vec![])
        .expect_err("an empty workspace export_dir should fail");
    assert!(
        error
            .to_string()
            .contains("workspace.export_dir cannot be empty"),
        "{error}"
    );
}

#[test]
fn an_empty_set_override_is_an_error() {
    let root = workspace_root();
    let error = resolve_with(
        &build_toml(&root, "", ""),
        vec![("workspace.export_dir", "")],
    )
    .expect_err("an empty --set export dir should fail");
    assert!(
        error
            .to_string()
            .contains("workspace.export_dir cannot be empty"),
        "{error}"
    );
}
