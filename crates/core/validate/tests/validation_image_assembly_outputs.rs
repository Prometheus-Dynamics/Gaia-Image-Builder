pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use support::write_temp_config;

const HEADER: &str = r#"
build_name = "outputs"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"
"#;

/// The flasher boot image: its one diagnostic is the warning that the boot
/// tree reads the initramfs the cpio-zstd step writes.
const FLASHER: &str = r#"
build_name = "flasher"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "flasher_defconfig"

[[image.assembly.trees]]
id = "initramfs"
path = "$assembly.work/initramfs"

[[image.assembly.trees]]
id = "boot"
path = "$assembly.work/boot"

[[image.assembly.busybox_initramfs]]
tree = "initramfs"
busybox = "$provider.target/bin/busybox"
applets = ["sh"]

[[image.assembly.kernel_modules]]
tree = "initramfs"
from = "$provider.target/lib/modules"
modules = ["libcomposite", "usb-f-mass-storage"]

[[image.assembly.filesystems]]
id = "initramfs"
kind = "cpio-zstd"
source_tree = "initramfs"
output = "$assembly.work/initramfs.cpio.zst"
compression_level = 19

[[image.assembly.files]]
tree = "boot"
src = "$assembly.work/initramfs.cpio.zst"
dest = "initramfs.cpio.zst"

[[image.assembly.filesystems]]
id = "boot"
kind = "vfat"
source_tree = "boot"
output = "$assembly.work/boot.img"
size = "64M"
publish = true
"#;

fn assembly_diagnostics(contents: &str) -> Vec<(&'static str, String)> {
    let path = write_temp_config(contents);
    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let report = validate_spec(&spec);
    let _ = fs::remove_file(path);
    report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code.starts_with("assembly_"))
        .map(|diagnostic| (diagnostic.code, diagnostic.message.clone()))
        .collect()
}

fn codes(contents: &str) -> Vec<&'static str> {
    assembly_diagnostics(contents)
        .into_iter()
        .map(|(code, _)| code)
        .collect()
}

#[test]
fn flasher_boot_image_validates_with_only_the_reads_later_output_warning() {
    assert_eq!(codes(FLASHER), vec!["assembly_reads_later_output"]);
}

#[test]
fn zstd_level_is_only_for_cpio_zstd_and_must_be_in_range() {
    let on_cpio = format!(
        r#"{HEADER}
[[image.assembly.trees]]
id = "t"
path = "$assembly.work/t"

[[image.assembly.filesystems]]
id = "c"
kind = "cpio"
source_tree = "t"
output = "$assembly.work/c.cpio"
compression_level = 3
"#
    );
    assert_eq!(
        codes(&on_cpio),
        vec!["assembly_filesystem_compression_level_unsupported"]
    );

    let too_high = format!(
        r#"{HEADER}
[[image.assembly.trees]]
id = "t"
path = "$assembly.work/t"

[[image.assembly.filesystems]]
id = "z"
kind = "cpio-zstd"
source_tree = "t"
output = "$assembly.work/z.cpio.zst"
compression_level = 20
"#
    );
    assert_eq!(
        codes(&too_high),
        vec!["assembly_filesystem_compression_level_invalid"]
    );
}

#[test]
fn cpio_zstd_is_deterministic_like_the_other_cpio_kinds() {
    let not_deterministic = format!(
        r#"{HEADER}
[[image.assembly.trees]]
id = "t"
path = "$assembly.work/t"

[[image.assembly.filesystems]]
id = "z"
kind = "cpio-zstd"
source_tree = "t"
output = "$assembly.work/z.cpio.zst"
deterministic = false
"#
    );
    assert_eq!(
        codes(&not_deterministic),
        vec!["assembly_filesystem_deterministic_unsupported"]
    );
}

#[test]
fn published_copy_may_not_land_on_another_output() {
    let disk_collision = format!(
        r#"{HEADER}
[[image.assembly.trees]]
id = "t"
path = "$assembly.work/t"

[[image.assembly.filesystems]]
id = "a"
kind = "cpio-zstd"
source_tree = "t"
output = "$assembly.work/a.cpio.zst"
publish = true

[[image.assembly.disks]]
id = "d"
output = "$provider.images/a.cpio.zst"
partition_table = "mbr"
"#
    );
    let found = assembly_diagnostics(&disk_collision);
    assert_eq!(
        found.iter().map(|(code, _)| *code).collect::<Vec<_>>(),
        vec!["assembly_filesystem_publish_collision"],
        "{found:?}"
    );
    assert!(found[0].1.contains("disk 'd'"), "{found:?}");

    let two_copies = format!(
        r#"{HEADER}
[[image.assembly.trees]]
id = "t"
path = "$assembly.work/t"

[[image.assembly.filesystems]]
id = "a"
kind = "cpio-zstd"
source_tree = "t"
output = "$assembly.work/a/x.cpio.zst"
publish = true

[[image.assembly.filesystems]]
id = "b"
kind = "cpio-zstd"
source_tree = "t"
output = "$assembly.work/b/x.cpio.zst"
publish = true
"#
    );
    // Each copy reports the collision on its own filesystem.
    assert_eq!(
        codes(&two_copies),
        vec![
            "assembly_filesystem_publish_collision",
            "assembly_filesystem_publish_collision"
        ]
    );
}

#[test]
fn kernel_modules_need_a_known_tree_modules_and_plain_names() {
    let bad = format!(
        r#"{HEADER}
[[image.assembly.trees]]
id = "t"
path = "$assembly.work/t"

[[image.assembly.kernel_modules]]
tree = "missing"
from = "$provider.target/lib/modules"
modules = ["libcomposite"]

[[image.assembly.kernel_modules]]
tree = "t"
from = "$provider.target/lib/modules"
modules = []

[[image.assembly.kernel_modules]]
tree = "t"
from = "$provider.target/lib/modules"
modules = ["lib/composite"]
kernel_version = "6.12/x"
"#
    );
    assert_eq!(
        codes(&bad),
        vec![
            "assembly_kernel_modules_tree_unknown",
            "assembly_kernel_modules_modules_empty",
            "assembly_kernel_modules_version_invalid",
            "assembly_kernel_modules_name_invalid",
        ]
    );
}
