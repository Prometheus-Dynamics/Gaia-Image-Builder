pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use support::write_temp_config;

#[test]
fn unknown_starting_point_source_is_rejected() {
    let path = write_temp_config(
        r#"
build_name = "unknown-starting-point-source"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
source = "missing-rootfs"
source_path = "rootfs"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "starting_point_source_unknown")
    );

    let _ = fs::remove_file(path);
}

#[test]
fn starting_point_output_contract_is_validated() {
    let path = write_temp_config(
        r#"
build_name = "invalid-starting-point-output"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"
output_mode = "copy-and-archive"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "starting_point_combined_output_incomplete")
    );

    let _ = fs::remove_file(path);
}

#[test]
fn starting_point_raw_image_contract_is_validated() {
    let path = write_temp_config(
        r#"
build_name = "invalid-starting-point-raw-image"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/base.img"
rootfs_validation_mode = "require-file"
output_mode = "archive-only"

[image.packages]
enabled = true
execute = true

[[artifacts]]
id = "gaia-app"
kind = "rust"
package = "gaia"
output_path = "out/gaia"

[[install]]
id = "install-gaia"
artifact = "gaia-app"
dest = "/usr/bin/gaia"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "starting_point_raw_image_requires_archive_name")
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "starting_point_raw_image_read_only_overlay")
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "starting_point_raw_image_read_only_packages")
    );

    let _ = fs::remove_file(path);
}
