pub mod support;

use gaia_config::resolve_config;
use gaia_spec::ImageDefinition;
use std::time::{SystemTime, UNIX_EPOCH};
use support::write_temp_config;

#[test]
fn resolves_buildroot_allow_fallback_override() {
    let path = write_temp_config(
        r#"
build_name = "buildroot-fallback-override"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"
"#,
    );

    let spec = gaia_config::resolve_config_with_options(
        path.to_str().expect("temp path should be utf-8"),
        &gaia_config::ResolveOptions {
            explicit_overrides: vec![("image.allow_fallback".into(), "true".into())],
            ..gaia_config::ResolveOptions::default()
        },
    );

    match &spec.image.definition {
        ImageDefinition::Buildroot(buildroot) => assert!(buildroot.allow_fallback),
        other => panic!("expected buildroot image, got {other:?}"),
    }

    let _ = std::fs::remove_file(path);
}

#[test]
fn resolves_source_backed_starting_point_image() {
    let path = write_temp_config(
        r#"
build_name = "starting-point-source"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "base-rootfs"
kind = "git"
repo = "https://example.invalid/base-rootfs.git"
branch = "main"

[image]
kind = "starting-point"
source = "base-rootfs"
source_path = "rootfs"
rootfs_validation_mode = "require-directory"
output_mode = "copy-and-archive"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));

    match &spec.image.definition {
        ImageDefinition::StartingPoint(starting_point) => {
            assert_eq!(
                starting_point.source.as_ref().map(|source| source.as_str()),
                Some("base-rootfs")
            );
            assert_eq!(starting_point.source_path.as_deref(), Some("rootfs"));
            assert!(starting_point.rootfs_path.is_empty());
        }
        definition => panic!("expected starting-point image, got {definition:?}"),
    }

    let _ = std::fs::remove_file(path);
}

#[test]
fn resolves_starting_point_raw_image_package_settings() {
    let path = write_temp_config(
        r#"
build_name = "starting-point-raw-image"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "base-image"
kind = "download"
url = "https://example.invalid/base.img"
output_path = "base.img"

[image]
kind = "starting-point"
source = "base-image"
source_path = "base.img"
rootfs_validation_mode = "require-file"
output_mode = "archive-only"
image_partition = "p2"
image_read_only = false

[image.packages]
enabled = true
execute = false
manager = "apt"
release_version = "24.04"
allow_major_upgrade = true
update = true
dist_upgrade = true
install = ["curl", "git"]
remove = ["nano"]
extra_args = ["--no-install-recommends"]
os_release_path = "/usr/lib/os-release"

[image.output]
archive_name = "base-mutated.img"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));

    match &spec.image.definition {
        ImageDefinition::StartingPoint(starting_point) => {
            assert_eq!(
                starting_point.source.as_ref().map(|source| source.as_str()),
                Some("base-image")
            );
            assert_eq!(starting_point.source_path.as_deref(), Some("base.img"));
            assert_eq!(starting_point.image_partition.as_deref(), Some("p2"));
            assert!(!starting_point.image_read_only);
            assert!(starting_point.packages.enabled);
            assert!(!starting_point.packages.execute);
            assert_eq!(starting_point.packages.manager.as_deref(), Some("apt"));
            assert_eq!(
                starting_point.packages.release_version.as_deref(),
                Some("24.04")
            );
            assert!(starting_point.packages.allow_major_upgrade);
            assert!(starting_point.packages.update);
            assert!(starting_point.packages.dist_upgrade);
            assert_eq!(
                starting_point.packages.install,
                vec!["curl".to_string(), "git".to_string()]
            );
            assert_eq!(starting_point.packages.remove, vec!["nano".to_string()]);
            assert_eq!(
                starting_point.packages.extra_args,
                vec!["--no-install-recommends".to_string()]
            );
            assert_eq!(
                starting_point.packages.os_release_path.as_deref(),
                Some("/usr/lib/os-release")
            );
        }
        definition => panic!("expected starting-point image, got {definition:?}"),
    }

    let _ = std::fs::remove_file(path);
}

#[test]
fn resolves_buildroot_config_fragments() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("gaia-buildroot-fragments-config-{nonce}"));
    std::fs::create_dir_all(root.join("assets")).expect("assets dir");
    std::fs::write(
        root.join("assets/base.defconfig"),
        "BR2_TARGET_ROOTFS_TAR=y\n",
    )
    .expect("base defconfig");
    std::fs::write(
        root.join("assets/fragment-a.cfg"),
        "BR2_PACKAGE_BUSYBOX=y\n",
    )
    .expect("fragment a");
    std::fs::write(
        root.join("assets/fragment-b.cfg"),
        "BR2_PACKAGE_DROPBEAR=y\n",
    )
    .expect("fragment b");

    let config_path = root.join("build.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
build_name = "buildroot-fragments"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig_path = "assets/base.defconfig"
allow_fallback = true
config_fragments = ["assets/fragment-a.cfg", "assets/fragment-b.cfg"]
config_overrides = [["BR2_TARGET_GENERIC_HOSTNAME", "\"gaia-smoke\""]]
"#,
            root.display()
        ),
    )
    .expect("config");

    let spec = resolve_config(config_path.to_str().expect("temp path should be utf-8"));

    match &spec.image.definition {
        ImageDefinition::Buildroot(buildroot) => {
            assert_eq!(
                buildroot.defconfig_path.as_deref(),
                Some("assets/base.defconfig")
            );
            assert!(buildroot.allow_fallback);
            assert_eq!(
                buildroot.config_fragments,
                vec![
                    "assets/fragment-a.cfg".to_string(),
                    "assets/fragment-b.cfg".to_string()
                ]
            );
            assert_eq!(
                buildroot.config_overrides,
                vec![(
                    "BR2_TARGET_GENERIC_HOSTNAME".to_string(),
                    "\"gaia-smoke\"".to_string()
                )]
            );
        }
        other => panic!("expected buildroot image, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(root);
}
