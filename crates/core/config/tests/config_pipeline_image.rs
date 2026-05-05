pub mod support;

use gaia_config::resolve_config;
use gaia_spec::{
    AssemblyFilesystemKindSpec, AssemblyTransformKindSpec, BuildrootExpectedImageFormatSpec,
    ImageDefinition,
};
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
fn resolves_image_assembly_tables() {
    let path = write_temp_config(
        r#"
build_name = "assembly-config"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.assembly]
work_dir = "build/assembly"
out_dir = "$provider.images"

[[image.assembly.trees]]
id = "boot"
path = "$assembly.work/boot"

[[image.assembly.trees]]
id = "initramfs"
path = "$assembly.work/initramfs"

[[image.assembly.dirs]]
tree = "initramfs"
path = "mnt/lower"
mode = "0755"

[[image.assembly.symlinks]]
tree = "initramfs"
path = "lib64"
target = "lib"

[[image.assembly.files]]
tree = "boot"
src = "@assets/board/config.txt"
dest = "config.txt"
mode = "0644"

[[image.assembly.files]]
tree = "boot"
src_glob = "$provider.images/*.dtb"
dest = "."
optional = true

[[image.assembly.transforms]]
kind = "gzip"
src = "$provider.images/Image"
dest = "$assembly.tree.boot/kernel.img"
deterministic = true

[[image.assembly.filesystems]]
id = "initramfs"
kind = "cpio-gzip"
source_tree = "initramfs"
output = "$assembly.tree.boot/initramfs"
deterministic = true

[[image.assembly.disks]]
id = "sdcard"
output = "$provider.images/sdcard.img"
partition_table = "mbr"
signature = "0x48454c49"
first_lba = 1
alignment_lba = 1

[[image.assembly.disks.partitions]]
name = "boot"
type_alias = "fat32-lba"
bootable = true
image = "$provider.images/boot.vfat"

[[image.assembly.busybox_initramfs]]
tree = "initramfs"
busybox = "$provider.target/usr/bin/busybox"
include_runtime_libs = true
applets = ["sh", "mount"]
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let assembly = spec.image.assembly.as_ref().expect("assembly");

    assert_eq!(assembly.work_dir.as_deref(), Some("build/assembly"));
    assert_eq!(assembly.out_dir.as_deref(), Some("$provider.images"));
    assert_eq!(assembly.trees.len(), 2);
    assert_eq!(assembly.dirs.len(), 1);
    assert_eq!(assembly.dirs[0].path, "mnt/lower");
    assert_eq!(assembly.dirs[0].mode.as_deref(), Some("0755"));
    assert_eq!(assembly.symlinks.len(), 1);
    assert_eq!(assembly.symlinks[0].path, "lib64");
    assert_eq!(assembly.symlinks[0].target, "lib");
    assert_eq!(assembly.files.len(), 2);
    assert_eq!(assembly.transforms[0].kind, AssemblyTransformKindSpec::Gzip);
    assert!(assembly.transforms[0].deterministic);
    assert_eq!(
        assembly.filesystems[0].kind,
        AssemblyFilesystemKindSpec::CpioGzip
    );
    assert!(assembly.filesystems[0].deterministic);
    assert_eq!(
        assembly.disks[0].partitions[0].type_alias.as_deref(),
        Some("fat32-lba")
    );
    assert_eq!(assembly.disks[0].first_lba, Some(1));
    assert_eq!(assembly.disks[0].alignment_lba, Some(1));
    assert_eq!(assembly.busybox_initramfs[0].applets, vec!["sh", "mount"]);

    let _ = std::fs::remove_file(path);
}

#[test]
fn image_assembly_deterministic_defaults_match_supported_operations() {
    let path = write_temp_config(
        r#"
build_name = "assembly-deterministic-defaults"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.assembly]
work_dir = "build/assembly"

[[image.assembly.transforms]]
kind = "gzip"
src = "$provider.images/Image"
dest = "$provider.images/Image.gz"

[[image.assembly.filesystems]]
id = "initramfs"
kind = "cpio"
source_tree = "boot"
output = "$provider.images/initramfs.cpio"

[[image.assembly.filesystems]]
id = "bootfs"
kind = "vfat"
source_tree = "boot"
output = "$provider.images/boot.vfat"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let assembly = spec.image.assembly.as_ref().expect("assembly");

    assert!(assembly.transforms[0].deterministic);
    assert!(assembly.filesystems[0].deterministic);
    assert!(!assembly.filesystems[1].deterministic);

    let _ = std::fs::remove_file(path);
}

#[test]
fn reporting_output_hygiene_policy_compiles() {
    let path = write_temp_config(
        r#"
build_name = "reporting-output-hygiene"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[reporting.output_hygiene]
large_file_threshold_bytes = 4096
transient_dir_names = [".cache", "tmp-work"]
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));

    assert_eq!(
        spec.reporting.output_hygiene.large_file_threshold_bytes,
        4096
    );
    assert_eq!(
        spec.reporting.output_hygiene.transient_dir_names,
        vec![".cache".to_string(), "tmp-work".to_string()]
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn image_configs_without_assembly_keep_assembly_absent() {
    for (name, image_toml) in [
        (
            "buildroot-no-assembly",
            r#"
[image]
kind = "buildroot"
defconfig = "dummy_defconfig"
"#,
        ),
        (
            "starting-point-no-assembly",
            r#"
[image]
kind = "starting-point"
rootfs_path = "/tmp/gaia-missing-rootfs"
rootfs_validation_mode = "allow-missing"
output_mode = "copy-rootfs"
"#,
        ),
    ] {
        let path = write_temp_config(&format!(
            r#"
build_name = "{name}"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"
{image_toml}
"#
        ));

        let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
        assert!(
            spec.image.assembly.is_none(),
            "{name} should not get an implicit assembly spec"
        );

        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn resolves_new_buildroot_expected_image_formats() {
    let path = write_temp_config(
        r#"
build_name = "buildroot-new-formats"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[[image.expected_images]]
name = "rootfs.cpio.gz"
format = "cpio"
required = true

[[image.expected_images]]
name = "rootfs.ext2"
format = "ext2"
required = true

[[image.expected_images]]
name = "rootfs.ext3"
format = "ext3"
required = true

[[image.expected_images]]
name = "rootfs.ubifs"
format = "ubifs"
required = true

[[image.expected_images]]
name = "rootfs.ubi"
format = "ubi"
required = true

[[image.expected_images]]
name = "rootfs.jffs2"
format = "jffs2"
required = true

[[image.expected_images]]
name = "rootfs.erofs"
format = "erofs"
required = true

[[image.expected_images]]
name = "rootfs.romfs"
format = "romfs"
required = true

[[image.expected_images]]
name = "rootfs.cramfs"
format = "cramfs"
required = true

[[image.expected_images]]
name = "rootfs.cloop"
format = "cloop"
required = true

[[image.expected_images]]
name = "rootfs.f2fs"
format = "f2fs"
required = true

[[image.expected_images]]
name = "rootfs.btrfs"
format = "btrfs"
required = true
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));

    match &spec.image.definition {
        ImageDefinition::Buildroot(buildroot) => {
            let mut formats = buildroot
                .expected_images
                .iter()
                .map(|image| image.format)
                .collect::<Vec<_>>();
            formats.sort_by_key(|format| format.as_str());
            assert_eq!(
                formats,
                vec![
                    BuildrootExpectedImageFormatSpec::Btrfs,
                    BuildrootExpectedImageFormatSpec::Cloop,
                    BuildrootExpectedImageFormatSpec::Cpio,
                    BuildrootExpectedImageFormatSpec::Cramfs,
                    BuildrootExpectedImageFormatSpec::Erofs,
                    BuildrootExpectedImageFormatSpec::Ext2,
                    BuildrootExpectedImageFormatSpec::Ext3,
                    BuildrootExpectedImageFormatSpec::F2fs,
                    BuildrootExpectedImageFormatSpec::Jffs2,
                    BuildrootExpectedImageFormatSpec::Romfs,
                    BuildrootExpectedImageFormatSpec::Ubi,
                    BuildrootExpectedImageFormatSpec::Ubifs,
                ]
            );
        }
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
