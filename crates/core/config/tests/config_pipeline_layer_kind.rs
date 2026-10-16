pub mod support;

use gaia_config::{resolve_config, try_resolve_config};
use gaia_spec::ImageDefinition;
use gaia_testkit::TempDir;
use std::fs;
use support::write_temp_config_at;

const ENTRY: &str = r#"
build_name = "layered"
extends = "layer.toml"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"
"#;

#[test]
fn layer_without_kind_merges_onto_a_build_file_that_sets_one() {
    let scratch = TempDir::new("gaia-config-layer-kind");
    let entry = scratch.join("config.toml");
    write_temp_config_at(&entry, ENTRY);
    fs::write(
        scratch.join("layer.toml"),
        r#"
[image.assembly]
work_dir = "build/assembly"

[[image.assembly.trees]]
id = "boot"
path = "$assembly.work/boot"
"#,
    )
    .expect("layer");

    let spec = resolve_config(entry.to_str().expect("utf-8 path"));

    assert!(matches!(
        spec.image.definition,
        ImageDefinition::Buildroot(_)
    ));
    let assembly = spec.image.assembly.as_ref().expect("layer assembly");
    assert_eq!(assembly.trees.len(), 1);
    assert_eq!(assembly.trees[0].id.as_str(), "boot");
}

#[test]
fn build_without_kind_anywhere_is_refused_with_a_clear_error() {
    let scratch = TempDir::new("gaia-config-no-kind");
    let entry = scratch.join("config.toml");
    write_temp_config_at(
        &entry,
        r#"
build_name = "nokind"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image.assembly]
work_dir = "build/assembly"
"#,
    );

    let error = try_resolve_config(entry.to_str().expect("utf-8 path"))
        .expect_err("a build without an image kind is refused");
    let message = error.to_string();
    assert!(message.contains("image.kind is not set"), "{message}");
}
