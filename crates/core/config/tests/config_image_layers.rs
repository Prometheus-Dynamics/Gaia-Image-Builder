pub mod support;

use gaia_spec::ImageDefinition;
use support::write_temp_config;

const BASE: &str = r#"
build_name = "image-layers"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"
"#;

fn resolve(text: &str) -> gaia_spec::ResolvedBuildSpec {
    let path = write_temp_config(text);
    gaia_config::resolve_config(path.to_str().expect("temp path should be utf-8"))
}

#[test]
fn layers_are_resolved_sorted_and_force_the_rootfs_tar() {
    let spec = resolve(&format!(
        "{BASE}\n[image.layers]\norder = [\"base\", \"platform\", \"app\"]\n\
         overrides = [\"/etc/x\"]\nverity = true\n\
         [image.layers.app]\npackages = [\"helios-*\"]\nstage = [\"raze-*\"]\n\
         [image.layers.platform]\npaths = [\"/lib/modules\"]\n"
    ));
    let layers = spec.image.layers.as_ref().expect("layers");
    assert_eq!(layers.order, ["base", "platform", "app"]);
    assert_eq!(layers.default, "base");
    assert!(layers.erofs && layers.verity);
    assert_eq!(layers.overrides, ["/etc/x"]);
    let names: Vec<&str> = layers
        .layers
        .iter()
        .map(|layer| layer.name.as_str())
        .collect();
    assert_eq!(names, ["app", "platform"]);
    assert_eq!(layers.layers[0].packages, ["helios-*"]);
    assert_eq!(layers.layers[1].paths, ["/lib/modules"]);
    match &spec.image.definition {
        ImageDefinition::Buildroot(buildroot) => assert!(
            buildroot
                .config_overrides
                .contains(&("BR2_TARGET_ROOTFS_TAR".to_string(), "y".to_string())),
            "{:?}",
            buildroot.config_overrides
        ),
        other => panic!("expected buildroot image, got {other:?}"),
    }
}

#[test]
fn without_layers_nothing_changes() {
    let spec = resolve(BASE);
    assert!(spec.image.layers.is_none());
    match &spec.image.definition {
        ImageDefinition::Buildroot(buildroot) => assert!(
            !buildroot
                .config_overrides
                .iter()
                .any(|(key, _)| key == "BR2_TARGET_ROOTFS_TAR")
        ),
        other => panic!("expected buildroot image, got {other:?}"),
    }
}
