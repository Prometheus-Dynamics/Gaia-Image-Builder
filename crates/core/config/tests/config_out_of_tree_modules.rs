//! `[[image.rootfs.out_of_tree_modules]]`: layers add modules (a later layer's
//! module of the same name replaces the earlier one), each module's package
//! is turned on unless an override sets it, and validation checks the names.

pub mod support;

use gaia_spec::ImageDefinition;
use gaia_testkit::TempDir;
use support::write_temp_config_at;

const DEVICE: &str = r#"
[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[[image.rootfs.out_of_tree_modules]]
name = "lemnos"
path = "old/lemnos"

[[image.rootfs.out_of_tree_modules]]
name = "wifi"
path = "${workspace.root_dir}/wifi"
"#;

const ENTRY: &str = r#"
build_name = "out-of-tree-modules"
imports = ["device.toml"]

[workspace]
root_dir = "/ws"
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
config_overrides = [["BR2_PACKAGE_GAIA_KMOD_WIFI", "n"]]

[[image.rootfs.out_of_tree_modules]]
name = "lemnos"
path = "lemnos/kernel-modules/lemnos"
"#;

#[test]
fn layers_add_modules_and_their_packages_are_turned_on() {
    let scratch = TempDir::new("gaia-out-of-tree-modules");
    write_temp_config_at(&scratch.join("device.toml"), DEVICE);
    let entry = scratch.join("build.toml");
    write_temp_config_at(&entry, ENTRY);
    let spec = gaia_config::resolve_config(entry.to_str().expect("temp path should be utf-8"));
    let ImageDefinition::Buildroot(buildroot) = &spec.image.definition else {
        panic!("expected a buildroot image");
    };
    let modules: Vec<_> = buildroot
        .rootfs_out_of_tree_modules
        .iter()
        .map(|module| (module.name.as_str(), module.path.as_str()))
        .collect();
    assert_eq!(
        modules,
        vec![
            ("lemnos", "lemnos/kernel-modules/lemnos"),
            ("wifi", "/ws/wifi")
        ]
    );
    let option = |name: &str| {
        buildroot
            .config_overrides
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(option("BR2_PACKAGE_GAIA_KMOD_LEMNOS"), vec!["y"]);
    // An override that sets the option wins.
    assert_eq!(option("BR2_PACKAGE_GAIA_KMOD_WIFI"), vec!["n"]);
}
