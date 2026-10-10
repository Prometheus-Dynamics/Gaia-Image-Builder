pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
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

fn assembly_codes(contents: &str) -> Vec<&'static str> {
    let path = write_temp_config(contents);
    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let report = validate_spec(&spec);
    let _ = fs::remove_file(path);
    report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.starts_with("assembly_"))
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn cm5_ab_layout_with_extended_partitions_and_bundle_validates() {
    assert_eq!(assembly_codes(CM5_AB_LAYOUT), Vec::<&str>::new());
}

#[test]
fn invalid_partition_sizes_levels_and_archives_are_reported() {
    let codes = assembly_codes(
        r#"
build_name = "invalid-layout"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[[image.assembly.transforms]]
kind = "zstd"
src = "$provider.images/rootfs.ext4"
dest = "$assembly.work/rootfs.ext4.zst"
level = 22

[[image.assembly.transforms]]
kind = "gzip"
src = "$provider.images/Image"
dest = "$assembly.work/Image.gz"
level = 9

[[image.assembly.disks]]
id = "emmc"
output = "$provider.images/emmc.img"

[[image.assembly.disks.partitions]]
name = "p1"
image = "$provider.images/a.img"
size = "lots"

[[image.assembly.disks.partitions]]
name = "p2"

[[image.assembly.disks.partitions]]
name = "p3"
image = "$provider.images/c.img"
wipe = true

[[image.assembly.disks.partitions]]
name = "p5"
bootable = true
size = "0"

[[image.assembly.disks.partitions]]
name = "p6"
size = "1M"

[[image.assembly.archives]]
id = "bundle"
output = "$provider.images/bundle.tar"

[[image.assembly.archives.members]]
name = "both"
src = "$provider.images/a.img"
entries = [["A", "b"]]

[[image.assembly.archives.members]]
name = "../escape"
src = "$provider.images/a.img"

[[image.assembly.archives.generated]]
name = "manifest.env"
entries = [["1BAD", "x"], ["DIGEST", "${assembly.md5:$provider.images/a.img}"], ["LEFT", "${not.a.token}"]]

[[image.assembly.archives]]
id = "bundle"
output = ""
"#,
    );

    for expected in [
        "assembly_transform_level_invalid",
        "assembly_transform_level_unsupported",
        "assembly_partition_size_invalid",
        "assembly_partition_image_or_size_required",
        "assembly_partition_wipe_with_image",
        "assembly_partition_bootable_logical",
        "assembly_archive_member_source_invalid",
        "assembly_archive_member_name_invalid",
        "assembly_archive_entry_key_invalid",
        "assembly_archive_entry_value_invalid",
        "assembly_archive_duplicate",
        "assembly_archive_output_empty",
        "assembly_archive_members_empty",
    ] {
        assert!(codes.contains(&expected), "missing {expected} in {codes:?}");
    }
    assert_eq!(
        codes
            .iter()
            .filter(|code| **code == "assembly_partition_size_invalid")
            .count(),
        2,
        "{codes:?}"
    );
    assert_eq!(
        codes
            .iter()
            .filter(|code| **code == "assembly_archive_entry_value_invalid")
            .count(),
        2,
        "{codes:?}"
    );
}

#[test]
fn long_archive_member_names_are_rejected() {
    let codes = assembly_codes(&format!(
        r#"
build_name = "long-name"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[[image.assembly.archives]]
id = "bundle"
output = "$provider.images/bundle.tar"

[[image.assembly.archives.members]]
name = "{}"
src = "$provider.images/a.img"
"#,
        "n".repeat(101)
    ));
    assert_eq!(codes, vec!["assembly_archive_member_name_invalid"]);
}

#[test]
fn transforms_reading_a_filesystem_built_later_are_reported() {
    // The boot filesystem the transform compresses is produced by the
    // assembly itself (the PhotonVision raze layout).
    let layout = format!(
        "{CM5_AB_LAYOUT}
[[image.assembly.trees]]
id = \"boot\"
path = \"$assembly.work/boot\"

[[image.assembly.filesystems]]
id = \"boot\"
kind = \"vfat\"
source_tree = \"boot\"
output = \"$provider.images/boot.vfat\"
size = \"128M\"
"
    );
    assert_eq!(assembly_codes(&layout), ["assembly_reads_later_output"]);
}

#[test]
fn assembly_steps_reading_each_others_outputs_are_an_error() {
    let layout = format!(
        "{CM5_AB_LAYOUT}
[[image.assembly.transforms]]
kind = \"copy\"
src = \"$assembly.work/a\"
dest = \"$assembly.work/b\"

[[image.assembly.transforms]]
kind = \"copy\"
src = \"$assembly.work/b\"
dest = \"$assembly.work/a\"
"
    );
    assert_eq!(assembly_codes(&layout), ["assembly_step_cycle"]);
}

/// One MBR or GPT disk with the given disk fields and partitions.
fn single_disk_codes(disk_fields: &str, partitions: &str) -> Vec<&'static str> {
    assembly_codes(&format!(
        r#"
build_name = "trunc"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[[image.assembly.disks]]
id = "emmc"
output = "$provider.images/emmc.img"
{disk_fields}
{partitions}
"#
    ))
}

#[test]
fn unmaterialized_partitions_reject_image_wipe_and_missing_size() {
    let codes = single_disk_codes(
        r#"partition_table = "mbr""#,
        r#"[[image.assembly.disks.partitions]]
name = "with-image"
type_alias = "linux"
image = "$provider.images/rootfs.ext4"
materialize = false
size = "2G"

[[image.assembly.disks.partitions]]
name = "wiped"
type_alias = "linux"
size = "1M"
wipe = true
materialize = false

[[image.assembly.disks.partitions]]
name = "no-size"
type_alias = "linux"
materialize = false
"#,
    );
    assert!(
        codes.contains(&"assembly_partition_unmaterialized_image"),
        "{codes:?}"
    );
    assert!(
        codes.contains(&"assembly_partition_unmaterialized_wipe"),
        "{codes:?}"
    );
    assert!(
        codes.contains(&"assembly_partition_unmaterialized_size_required"),
        "{codes:?}"
    );
}

#[test]
fn truncate_is_rejected_for_gpt_and_accepted_for_mbr() {
    let partitions = r#"[[image.assembly.disks.partitions]]
name = "data"
type_alias = "linux"
size = "16M"
materialize = false
"#;
    let gpt = single_disk_codes(
        r#"partition_table = "gpt"
truncate = "last-data""#,
        partitions,
    );
    assert!(
        gpt.contains(&"assembly_disk_truncate_partition_table_unsupported"),
        "{gpt:?}"
    );
    let mbr = single_disk_codes(
        r#"partition_table = "mbr"
truncate = "last-data"
ebr_placement = "packed""#,
        partitions,
    );
    assert_eq!(mbr, Vec::<&str>::new());
}
