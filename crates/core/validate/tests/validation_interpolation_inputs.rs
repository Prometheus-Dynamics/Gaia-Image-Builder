pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use support::write_temp_config;

#[test]
fn unresolved_interpolation_is_a_warning_when_allowed() {
    let path = write_temp_config(
        r#"
build_name = "unresolved-allowed"
description = "hello ${missing.token}"

[interpolation]
allow_unresolved = true

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[image.feed]
install_entries = []
stage_files = []
stage_env_sets = []
stage_services = []
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    assert!(
        spec.policy
            .interpolation
            .unresolved
            .iter()
            .any(|entry| entry.location == "build.description" && entry.token == "missing.token")
    );
    assert!(report.errors.is_empty());
    assert!(
        report
            .warnings
            .iter()
            .any(|message| message.contains("unresolved interpolation token '${missing.token}'"))
    );

    let _ = fs::remove_file(path);
}

#[test]
fn unresolved_interpolation_is_an_error_when_disallowed() {
    let path = write_temp_config(
        r#"
build_name = "unresolved-disallowed"
description = "hello ${missing.token}"

[interpolation]
allow_unresolved = false

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"

[image.feed]
install_entries = []
stage_files = []
stage_env_sets = []
stage_services = []
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    assert!(
        spec.policy
            .interpolation
            .unresolved
            .iter()
            .any(|entry| entry.location == "build.description" && entry.token == "missing.token")
    );
    assert!(
        report
            .errors
            .iter()
            .any(|message| message.contains("unresolved interpolation token '${missing.token}'"))
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "interpolation_unresolved"
                && diagnostic.location.as_deref() == Some("build.description"))
    );

    let _ = fs::remove_file(path);
}

#[test]
fn required_and_enum_inputs_are_validated() {
    let path = write_temp_config(
        r#"
build_name = "invalid-inputs"

[inputs.target]
kind = "enum"
required = true
choices = ["cm5", "rpi5"]

[inputs.jobs]
kind = "integer"
default = "two"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "required_input_missing"
                && diagnostic.location.as_deref() == Some("input:target"))
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "input_integer_invalid"
                && diagnostic.location.as_deref() == Some("input:jobs"))
    );

    let _ = fs::remove_file(path);
}
