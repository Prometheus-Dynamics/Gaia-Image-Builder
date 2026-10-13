pub mod support;

use gaia_spec::ImageDefinition;
use support::write_temp_config;

const BASE: &str = r#"
build_name = "rootfs-exclude"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"
"#;

fn excluded(spec: &gaia_spec::ResolvedBuildSpec) -> Vec<String> {
    match &spec.image.definition {
        ImageDefinition::Buildroot(buildroot) => buildroot.rootfs_exclude_packages.clone(),
        other => panic!("expected buildroot image, got {other:?}"),
    }
}

#[test]
fn resolves_the_image_rootfs_table() {
    let path = write_temp_config(&format!(
        "{BASE}\n[image.rootfs]\nexclude_packages = [\"libcamera\", \"libpisp\"]\n"
    ));
    let spec = gaia_config::resolve_config(path.to_str().expect("temp path should be utf-8"));
    assert_eq!(excluded(&spec), vec!["libcamera", "libpisp"]);
}

#[test]
fn the_override_sets_the_exclusion_list() {
    let path = write_temp_config(BASE);
    let spec = gaia_config::resolve_config_with_options(
        path.to_str().expect("temp path should be utf-8"),
        &gaia_config::ResolveOptions {
            explicit_overrides: vec![(
                "image.rootfs.exclude_packages".into(),
                "libcamera, libpisp".into(),
            )],
            ..gaia_config::ResolveOptions::default()
        },
    );
    assert_eq!(excluded(&spec), vec!["libcamera", "libpisp"]);
}
