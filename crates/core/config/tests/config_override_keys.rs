//! `--set` keys that name no setting, and `input.<name>` keys for inputs the
//! build does not declare, are errors rather than silently ignored.

pub mod support;

use gaia_config::{ConfigError, ResolveOptions, try_resolve_config_with_options};
use support::write_temp_config;

const BUILD: &str = r#"
build_name = "override-keys"
version = "1.0.0"

[inputs.backend]
kind = "string"
default = "libcamera"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "rootfs"

[presets.bad]
overrides = [["image.rootfs_pth", "/x"]]
"#;

fn resolve(options: ResolveOptions) -> Result<(), ConfigError> {
    let config = write_temp_config(BUILD);
    try_resolve_config_with_options(config.as_path().to_str().expect("utf-8 path"), &options)
        .map(|_| ())
}

fn with(overrides: &[(&str, &str)]) -> ResolveOptions {
    ResolveOptions {
        explicit_overrides: overrides
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        ..ResolveOptions::default()
    }
}

#[test]
fn misspelled_set_key_is_an_error_with_a_suggestion() {
    let error = resolve(with(&[("image.rootfs_pth", "/x")]))
        .expect_err("a misspelled --set key must not be ignored");

    assert!(matches!(error, ConfigError::InvalidOverrideKey { .. }));
    let message = error.to_string();
    assert!(
        message.contains("image.rootfs_pth")
            && message.contains("'image.starting-point.rootfs_path'"),
        "unexpected message: {message}"
    );
}

#[test]
fn set_key_naming_no_setting_is_an_error() {
    let error = resolve(with(&[("zzzz.qqqq", "1")])).expect_err("an unknown --set key must fail");

    assert!(error.to_string().contains("names no setting"));
}

#[test]
fn set_input_for_undeclared_input_is_an_error() {
    let error = resolve(with(&[("input.slot_commit", "abc")]))
        .expect_err("an input the build does not declare must not be ignored");

    assert!(
        error
            .to_string()
            .contains("declares no input 'slot_commit'")
    );
}

#[test]
fn set_input_for_declared_input_resolves() {
    resolve(with(&[("input.backend", "styx")])).expect("declared input is accepted");
}

#[test]
fn preset_with_unknown_key_is_an_error() {
    let error = resolve(ResolveOptions {
        preset: Some("bad".to_string()),
        ..ResolveOptions::default()
    })
    .expect_err("a preset naming an unknown key must fail");

    assert!(
        error
            .to_string()
            .contains("'image.starting-point.rootfs_path'")
    );
}
