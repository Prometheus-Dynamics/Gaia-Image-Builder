pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use support::write_temp_config;

/// A root tree of one file, with the given assembly sections appended.
fn assembly(sections: &str) -> String {
    format!(
        r#"
build_name = "acme"
version = "1.2.3"

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
    )
}

fn assembly_messages(contents: &str) -> Vec<(&'static str, String)> {
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

#[test]
fn a_verity_slot_set_on_a_flash_disk_validates() {
    let contents = assembly(
        r#"
[[image.assembly.transforms]]
kind = "verity"
src = "$provider.images/rootfs.erofs"
dest = "$assembly.work/rootfs.verity.erofs"

[[image.assembly.ubis]]
id = "nand"
output = "$assembly.work/nand.ubi"
peb_size = "128K"
min_io_size = "2K"
flash_size = "128M"

[[image.assembly.ubis.volumes]]
name = "rootfs_a"
image = "$assembly.work/rootfs.verity.erofs"
size = "40M"

[[image.assembly.ubis.volumes]]
name = "rootfs_b"
size = "40M"

[[image.assembly.ubis.volumes]]
name = "data"
type = "dynamic"
size = "1M"
autoresize = true

[[image.assembly.disks]]
id = "flash"
output = "$assembly.out/flash.img"
partition_table = "gpt"

[[image.assembly.disks.partitions]]
name = "ubi"
image = "$assembly.work/nand.ubi"
offset = "4M"
table = false
"#,
    );

    assert_eq!(assembly_messages(&contents), Vec::new());
}

#[test]
fn bad_geometry_volume_sets_and_images_are_reported() {
    let contents = assembly(
        r#"
[[image.assembly.ubis]]
id = "nand"
output = "$assembly.out/nand.ubi"
peb_size = "100K"
min_io_size = "2K"
volumes = [{ name = "a", size = "1M" }]

[[image.assembly.ubis]]
id = "nand"
output = "$assembly.out/other.ubi"
peb_size = "128K"
min_io_size = "2K"
flash_size = "big"
bad_pebs_per_1024 = 800

[[image.assembly.ubis.volumes]]
name = "a"
image = "$assembly.work/missing.img"
autoresize = true

[[image.assembly.ubis.volumes]]
name = "a"
id = 0
autoresize = true

[[image.assembly.ubis.volumes]]
name = "empty"
type = "static"

[[image.assembly.ubis]]
id = "none"
output = "$assembly.out/none.ubi"
peb_size = "128K"
min_io_size = "2K"
"#,
    );

    let messages = assembly_messages(&contents);
    let codes: Vec<&str> = messages.iter().map(|(code, _)| *code).collect();
    assert_eq!(
        codes,
        [
            "assembly_ubi_geometry_invalid",
            "assembly_ubi_duplicate",
            "assembly_ubi_volume_invalid",
            "assembly_ubi_volume_invalid",
            "assembly_ubi_volume_invalid",
            "assembly_ubi_volume_invalid",
            "assembly_ubi_flash_size_invalid",
            "assembly_ubi_bad_pebs_invalid",
            "assembly_ubi_volume_image_unknown",
            "assembly_ubi_volumes_empty",
        ],
        "{messages:#?}"
    );
    assert!(messages[0].1.contains("must be powers of two"));
    assert!(messages[2].1.contains("only one volume can be autoresize"));
    assert!(messages[3].1.contains("share id 0"));
    assert!(messages[4].1.contains("two volumes are named 'a'"));
    assert!(
        messages[5]
            .1
            .contains("volume 'empty' has no image, size or autoresize")
    );
}
