//! `[image.rootfs] overlays`: each layer's directories are appended to
//! `BR2_ROOTFS_OVERLAY`, after a `config_overrides` value of it.

pub mod support;

use gaia_spec::ImageDefinition;
use gaia_testkit::TempDir;
use support::write_temp_config_at;

const DEVICE: &str = r#"
[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.rootfs]
overlays = ["${workspace.root_dir}/device-overlay"]
"#;

const ENTRY: &str = r#"
build_name = "rootfs-overlays"
imports = ["device.toml"]

[workspace]
root_dir = "/ws"
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
config_overrides = [["BR2_ROOTFS_OVERLAY", "\"board/os-overlay\""]]

[image.rootfs]
overlays = ["os-extra", "/ws/device-overlay"]
"#;

#[test]
fn layers_append_their_overlays_to_the_override() {
    let scratch = TempDir::new("gaia-rootfs-overlays");
    write_temp_config_at(&scratch.join("device.toml"), DEVICE);
    let entry = scratch.join("build.toml");
    write_temp_config_at(&entry, ENTRY);
    let spec = gaia_config::resolve_config(entry.to_str().expect("temp path should be utf-8"));
    let ImageDefinition::Buildroot(buildroot) = &spec.image.definition else {
        panic!("expected a buildroot image");
    };
    let overlay: Vec<_> = buildroot
        .config_overrides
        .iter()
        .filter(|(key, _)| key == "BR2_ROOTFS_OVERLAY")
        .map(|(_, value)| value.as_str())
        .collect();
    assert_eq!(
        overlay,
        vec!["\"board/os-overlay /ws/device-overlay os-extra\""]
    );
}
