pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use support::write_temp_config;

/// A root tree of one file, with the given assembly sections appended.
fn assembly_codes(sections: &str) -> Vec<&'static str> {
    let path = write_temp_config(&format!(
        r#"
build_name = "acme"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.assembly]
work_dir = "${{workspace.build_dir}}/assembly"

[[image.assembly.trees]]
id = "rootfs"
path = "$assembly.work/rootfs"

[[image.assembly.files]]
tree = "rootfs"
src = "@assets/motd.txt"
dest = "etc/motd"
{sections}
"#
    ));
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

fn filesystem(kind: &str, options: &str) -> String {
    format!(
        r#"
[[image.assembly.filesystems]]
id = "root"
kind = "{kind}"
source_tree = "rootfs"
output = "$assembly.work/root.img"
{options}
"#
    )
}

#[test]
fn squashfs_and_erofs_options_validate() {
    for sections in [
        filesystem("squashfs", ""),
        filesystem(
            "squashfs",
            "compression = \"zstd\"\ncompression_level = 22\nblock_size = 1048576\npublish = true",
        ),
        filesystem("erofs", ""),
        filesystem(
            "erofs",
            "compression = \"lz4hc\"\ncompression_level = 12\npcluster_size = 65536\ndedupe = true\nfragments = true\nztailpacking = true\nlabel = \"root\"\nuuid = \"6e2d2b1c-0000-4000-8000-00000000abcd\"",
        ),
        filesystem("erofs", "compression = \"none\"\nblock_size = 4096"),
    ] {
        assert_eq!(assembly_codes(&sections), Vec::<&str>::new(), "{sections}");
    }
}

#[test]
fn squashfs_and_erofs_options_are_checked() {
    let cases = [
        (
            filesystem("squashfs", "compression = \"lzma\""),
            "assembly_filesystem_compression_invalid",
        ),
        (
            filesystem("squashfs", "compression = \"xz\"\ncompression_level = 6"),
            "assembly_filesystem_compression_level_unsupported",
        ),
        (
            filesystem("squashfs", "compression_level = 10"),
            "assembly_filesystem_compression_level_invalid",
        ),
        (
            filesystem("squashfs", "block_size = 1000"),
            "assembly_filesystem_block_size_invalid",
        ),
        (
            filesystem("squashfs", "dedupe = true"),
            "assembly_filesystem_option_unsupported",
        ),
        (
            filesystem("squashfs", "size = \"64M\""),
            "assembly_filesystem_option_unsupported",
        ),
        (
            filesystem("squashfs", "size = \"auto\""),
            "assembly_filesystem_size_auto_unsupported",
        ),
        (
            filesystem("squashfs", "label = \"root\""),
            "assembly_filesystem_option_unsupported",
        ),
        (
            filesystem("squashfs", "deterministic = false"),
            "assembly_filesystem_deterministic_unsupported",
        ),
        (
            filesystem("erofs", "compression = \"zstd\""),
            "assembly_filesystem_compression_invalid",
        ),
        (
            filesystem("erofs", "compression = \"lz4\"\ncompression_level = 3"),
            "assembly_filesystem_compression_level_unsupported",
        ),
        (
            filesystem("erofs", "compression = \"lzma\"\ncompression_level = 10"),
            "assembly_filesystem_compression_level_invalid",
        ),
        (
            filesystem("erofs", "pcluster_size = 6000"),
            "assembly_filesystem_pcluster_size_invalid",
        ),
        (
            filesystem("erofs", "compression = \"none\"\nfragments = true"),
            "assembly_filesystem_option_unsupported",
        ),
        (
            filesystem("erofs", "uuid = \"nope\""),
            "assembly_filesystem_uuid_invalid",
        ),
        (
            filesystem("erofs", "journal = false"),
            "assembly_filesystem_option_unsupported",
        ),
        (
            filesystem("ext4", "size = \"16M\"\ncompression = \"gzip\""),
            "assembly_filesystem_option_unsupported",
        ),
        (
            filesystem("cpio-zstd", "ztailpacking = true"),
            "assembly_filesystem_option_unsupported",
        ),
    ];
    for (sections, code) in cases {
        let codes = assembly_codes(&sections);
        assert!(codes.contains(&code), "{code} for {sections}: {codes:?}");
    }
}

fn disk(fields: &str, partitions: &str) -> String {
    format!(
        r#"
[[image.assembly.disks]]
id = "disk"
output = "$assembly.out/disk.img"
{fields}
{partitions}
"#
    )
}

const GPT_PARTITIONS: &str = r#"
[[image.assembly.disks.partitions]]
name = "ESP"
type_alias = "esp"
bootable = true
size = "64M"

[[image.assembly.disks.partitions]]
name = "root"
type_alias = "linux-root-arm64"
guid = "fedcba98-7654-4321-8fed-cba987654321"
size = "1G"

[[image.assembly.disks.partitions]]
name = "data"
type = "0FC63DAF-8483-4772-8E79-3D69D8477DE4"
size = "16M"

[[image.assembly.disks.partitions]]
name = "four"
size = "1M"

[[image.assembly.disks.partitions]]
name = "five"
bootable = true
size = "1M"
"#;

#[test]
fn gpt_disks_validate() {
    let codes = assembly_codes(&disk(
        "partition_table = \"gpt\"\ndisk_guid = \"01234567-89ab-4cde-8f01-23456789abcd\"\nfirst_lba = 34",
        GPT_PARTITIONS,
    ));
    // Five partitions, the fifth bootable: no extended layout on GPT.
    assert_eq!(codes, Vec::<&str>::new());
}

#[test]
fn gpt_and_mbr_options_are_checked() {
    let cases = [
        (
            disk(
                "partition_table = \"gpt\"\nsignature = \"0x12345678\"",
                GPT_PARTITIONS,
            ),
            "assembly_disk_option_unsupported",
        ),
        (
            disk(
                "partition_table = \"gpt\"\nebr_placement = \"packed\"",
                GPT_PARTITIONS,
            ),
            "assembly_disk_option_unsupported",
        ),
        (
            disk("partition_table = \"gpt\"\nfirst_lba = 16", GPT_PARTITIONS),
            "assembly_disk_first_lba_invalid",
        ),
        (
            disk(
                "partition_table = \"gpt\"\ndisk_guid = \"not-a-guid\"",
                GPT_PARTITIONS,
            ),
            "assembly_disk_guid_invalid",
        ),
        (
            disk(
                "partition_table = \"gpt\"",
                "[[image.assembly.disks.partitions]]\nname = \"a\"\ntype_alias = \"fat32-lba\"\nsize = \"1M\"",
            ),
            "assembly_partition_type_alias_invalid",
        ),
        (
            disk(
                "partition_table = \"gpt\"",
                "[[image.assembly.disks.partitions]]\nname = \"a\"\ntype = \"0x83\"\nsize = \"1M\"",
            ),
            "assembly_partition_type_raw_invalid",
        ),
        (
            disk(
                "partition_table = \"gpt\"",
                "[[image.assembly.disks.partitions]]\nname = \"a\"\nguid = \"x\"\nsize = \"1M\"",
            ),
            "assembly_partition_guid_invalid",
        ),
        (
            disk(
                "partition_table = \"gpt\"",
                "[[image.assembly.disks.partitions]]\nname = \"a\"\nsize = \"1M\"\n[[image.assembly.disks.partitions]]\nname = \"a\"\nsize = \"1M\"",
            ),
            "assembly_partition_name_duplicate",
        ),
        (
            disk(
                "partition_table = \"gpt\"",
                &format!(
                    "[[image.assembly.disks.partitions]]\nname = \"{}\"\nsize = \"1M\"",
                    "n".repeat(37)
                ),
            ),
            "assembly_partition_name_invalid",
        ),
        (
            disk(
                "partition_table = \"mbr\"\ndisk_guid = \"01234567-89ab-4cde-8f01-23456789abcd\"",
                "[[image.assembly.disks.partitions]]\nname = \"a\"\nsize = \"1M\"",
            ),
            "assembly_disk_option_unsupported",
        ),
        (
            disk(
                "partition_table = \"mbr\"",
                "[[image.assembly.disks.partitions]]\nname = \"a\"\nguid = \"01234567-89ab-4cde-8f01-23456789abcd\"\nsize = \"1M\"",
            ),
            "assembly_disk_option_unsupported",
        ),
    ];
    for (sections, code) in cases {
        let codes = assembly_codes(&sections);
        assert!(codes.contains(&code), "{code} for {sections}: {codes:?}");
    }
}
