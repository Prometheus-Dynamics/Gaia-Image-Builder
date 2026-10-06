pub mod support;

use gaia_config::resolve_config;
use gaia_spec::AssemblyTransformKindSpec;
use support::write_temp_config;

/// The CM5 A/B layout from docs/configuration.md.
const CM5_AB_LAYOUT: &str = r#"
build_name = "helios"
version = "1.2.3"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.assembly]
work_dir = "${workspace.build_dir}/assembly"

[[image.assembly.trees]]
id = "autoboot"
path = "$assembly.work/autoboot"

[[image.assembly.files]]
tree = "autoboot"
src = "@assets/autoboot.txt"
dest = "autoboot.txt"

[[image.assembly.filesystems]]
id = "autoboot"
kind = "vfat"
source_tree = "autoboot"
output = "$provider.images/autoboot.vfat"
size = "16M"

[[image.assembly.transforms]]
kind = "zstd"
src = "$provider.images/boot.vfat"
dest = "$assembly.work/boot.vfat.zst"

[[image.assembly.transforms]]
kind = "zstd"
src = "$provider.images/rootfs.ext4"
dest = "$assembly.work/rootfs.ext4.zst"
level = 19

[[image.assembly.disks]]
id = "emmc"
output = "$provider.images/emmc.img"
partition_table = "mbr"

[[image.assembly.disks.partitions]]
name = "autoboot"
type_alias = "fat32-lba"
bootable = true
image = "$provider.images/autoboot.vfat"
size = "16M"

[[image.assembly.disks.partitions]]
name = "boot-a"
type_alias = "fat32-lba"
image = "$provider.images/boot.vfat"
size = "128M"

[[image.assembly.disks.partitions]]
name = "boot-b"
type_alias = "fat32-lba"
image = "$provider.images/boot.vfat"
size = "128M"

[[image.assembly.disks.partitions]]
name = "rootfs-a"
type_alias = "linux"
image = "$provider.images/rootfs.ext4"
size = "2G"

[[image.assembly.disks.partitions]]
name = "rootfs-b"
type_alias = "linux"
size = "2G"
wipe = true

[[image.assembly.disks.partitions]]
name = "data"
type_alias = "linux"
image = "$provider.images/data.ext4"

[[image.assembly.archives]]
id = "update"
output = "$provider.images/${build.name}-${build.version}.pdupdate"

[[image.assembly.archives.members]]
name = "manifest.env"
entries = [
  ["MODEL", "cm5"],
  ["VERSION", "${build.version}"],
  ["OS", "HeliOS"],
  ["BOOT_SHA256", "${assembly.sha256:$assembly.work/boot.vfat.zst}"],
  ["ROOTFS_SHA256", "${assembly.sha256:$assembly.work/rootfs.ext4.zst}"],
]

[[image.assembly.archives.members]]
name = "boot.vfat.zst"
src = "$assembly.work/boot.vfat.zst"

[[image.assembly.archives.members]]
name = "rootfs.ext4.zst"
src = "$assembly.work/rootfs.ext4.zst"
"#;

#[test]
fn resolves_extended_partitions_zstd_and_archives() {
    let path = write_temp_config(CM5_AB_LAYOUT);
    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let _ = std::fs::remove_file(path);
    let assembly = spec.image.assembly.as_ref().expect("assembly");

    assert!(
        spec.policy.interpolation.unresolved.is_empty(),
        "{:?}",
        spec.policy.interpolation.unresolved
    );
    assert_eq!(assembly.transforms[0].kind, AssemblyTransformKindSpec::Zstd);
    assert_eq!(assembly.transforms[0].level, None);
    assert_eq!(assembly.transforms[1].level, Some(19));
    let partitions = &assembly.disks[0].partitions;
    assert_eq!(partitions.len(), 6);
    assert_eq!(partitions[1].size.as_deref(), Some("128M"));
    assert_eq!(partitions[4].image, None);
    assert_eq!(partitions[4].size.as_deref(), Some("2G"));
    assert!(partitions[4].wipe);
    assert_eq!(
        partitions[4]
            .parsed_size()
            .expect("size")
            .expect("set")
            .bytes(),
        2 * 1024 * 1024 * 1024
    );
    assert_eq!(partitions[5].size, None);

    let archive = &assembly.archives[0];
    assert_eq!(
        archive.output.as_str(),
        "$provider.images/helios-1.2.3.pdupdate"
    );
    assert_eq!(
        archive
            .members
            .iter()
            .map(|member| member.name.as_str())
            .collect::<Vec<_>>(),
        vec!["manifest.env", "boot.vfat.zst", "rootfs.ext4.zst"]
    );
    let entries = archive.members[0].entries.as_ref().expect("entries");
    assert_eq!(entries[1], ("VERSION".into(), "1.2.3".into()));
    assert_eq!(
        entries[3],
        (
            "BOOT_SHA256".into(),
            "${assembly.sha256:$assembly.work/boot.vfat.zst}".into()
        )
    );
}

#[test]
fn archive_generated_members_come_first_and_digest_paths_interpolate() {
    let path = write_temp_config(
        r#"
build_name = "bundle"
version = "9.9"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[[image.assembly.archives]]
id = "update"
output = "$provider.images/update.tar"

[[image.assembly.archives.members]]
name = "rootfs.ext4"
src = "$provider.images/rootfs.ext4"

[[image.assembly.archives.generated]]
name = "manifest.env"
entries = [["IMAGE_SHA256", "v${build.version}:${assembly.sha256:$provider.images/${build.name}-${build.version}.img}"]]
"#,
    );
    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let _ = std::fs::remove_file(path);
    let archive = &spec.image.assembly.as_ref().expect("assembly").archives[0];

    assert_eq!(archive.members[0].name, "manifest.env");
    assert_eq!(archive.members[0].src, None);
    assert_eq!(
        archive.members[0].entries.as_deref(),
        Some(
            &[(
                "IMAGE_SHA256".to_string(),
                "v9.9:${assembly.sha256:$provider.images/bundle-9.9.img}".to_string()
            )][..]
        )
    );
    assert_eq!(archive.members[1].name, "rootfs.ext4");
    assert_eq!(archive.members[1].entries, None);
}
