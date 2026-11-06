pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use support::write_temp_config;

const BASE: &str = r#"
build_name = "crate-splits"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "/tmp/rootfs"
"#;

fn split_codes(splits: &str) -> Vec<String> {
    let path = write_temp_config(&format!("{BASE}\n{splits}"));
    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);
    let _ = fs::remove_file(path);
    report
        .diagnostics
        .into_iter()
        .filter(|diagnostic| diagnostic.code.starts_with("image_crate_split"))
        .map(|diagnostic| diagnostic.code.to_string())
        .collect()
}

#[test]
fn an_allowed_crate_split_names_a_crate_and_a_reason() {
    assert!(split_codes(
        "[[image.checks.crate_splits]]\ncrate = \"daedalus-accel\"\nreason = \"Eidos pins its own accel\"\n"
    )
    .is_empty());
    assert_eq!(
        split_codes(
            "[[image.checks.crate_splits]]\ncrate = \"daedalus-accel\"\n\n[[image.checks.crate_splits]]\ncrate = \" \"\nreason = \"why\"\n"
        ),
        [
            "image_crate_split_reason_missing",
            "image_crate_split_crate_missing"
        ]
    );
}
