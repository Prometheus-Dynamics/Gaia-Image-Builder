//! `${source.<id>.path}` in Buildroot `config_overrides` makes the image
//! wait for that source.

pub mod support;

use gaia_config::resolve_config;
use gaia_plan::plan_build;
use gaia_spec::ImageDefinition;
use std::fs;
use std::path::PathBuf;
use support::{provider_catalogs, unique_dir};

#[test]
fn image_depends_on_sources_named_by_config_overrides() {
    let root_dir = unique_dir("gaia-plan-source-paths");
    fs::create_dir_all(PathBuf::from(&root_dir).join("tables")).expect("root dir");
    let config_path = PathBuf::from(&root_dir).join("build.toml");
    fs::write(
        &config_path,
        r#"
build_name = "source-paths"

[workspace]
root_dir = "@WORKSPACE@"
build_dir = "build"
out_dir = "out"

[[sources]]
id = "buildroot-source"
kind = "path"
path = "."

[[sources]]
id = "orion"
kind = "git"
repo = "https://example.invalid/orion.git"

[[sources]]
id = "orion-extra"
kind = "git"
repo = "https://example.invalid/orion-extra.git"

[[sources]]
id = "tables"
kind = "path"
path = "tables"

[[artifacts]]
id = "gaia-app"
kind = "rust"
package = "gaia"
after_image_prepare = true
output_path = "out/gaia"

[image]
kind = "buildroot"
source = "buildroot-source"
defconfig = "qemu_aarch64_virt_defconfig"
config_overrides = [
  ["BR2_ROOTFS_USERS_TABLES", "\"${source.orion-extra.path}/packaging/users.table\""],
  ["BR2_ROOTFS_DEVICE_TABLE", "\"system/device_table.txt ${source.tables.path}/device.table\""],
]
"#
        .replace("@WORKSPACE@", &root_dir),
    )
    .expect("config");

    let spec = resolve_config(config_path.to_str().expect("utf-8 config path"));
    let ImageDefinition::Buildroot(buildroot) = &spec.image.definition else {
        panic!("buildroot image");
    };
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let users_table = buildroot
        .config_overrides
        .iter()
        .find(|(key, _)| key == "BR2_ROOTFS_USERS_TABLES")
        .map(|(_, value)| value.clone());
    assert_eq!(
        users_table.as_deref().unwrap_or_default(),
        format!(
            "\"{}/packaging/users.table\"",
            build_dir.join("sources/orion-extra").display()
        )
        .as_str()
    );
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    for id in ["image:prepare", "image:build"] {
        let operation = plan
            .operations
            .iter()
            .find(|operation| operation.id.as_str() == id)
            .expect("image operation");
        let depends_on = |source: &str| {
            operation
                .depends_on
                .iter()
                .any(|dependency| dependency.as_str() == source)
        };
        assert!(depends_on("source:orion-extra"), "{id}");
        assert!(depends_on("source:tables"), "{id}");
        assert!(
            !depends_on("source:orion"),
            "a source whose directory is only a prefix of another's is not named: {id}"
        );
    }
    let _ = fs::remove_dir_all(root_dir);
}
