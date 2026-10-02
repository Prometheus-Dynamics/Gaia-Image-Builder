pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use support::write_temp_config;

fn archive_warning(expected_images: &str, assembly: &str) -> bool {
    let path = write_temp_config(&format!(
        r#"
build_name = "archive-check"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "raspberrypi4_64_defconfig"

[image.output]
collect_dir = "out/images"
archive_name = "board.img"

{expected_images}
{assembly}
"#
    ));
    let spec = resolve_config(path.to_str().expect("utf-8 path"));
    let _ = std::fs::remove_file(path);
    validate_spec(&spec)
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "image_archive_not_a_disk")
}

#[test]
fn img_archive_of_a_rootfs_image_is_a_warning() {
    assert!(archive_warning(
        "[[image.expected_images]]\nname = \"rootfs.ext2\"\nformat = \"ext2\"\nrequired = true\n",
        "",
    ));
}

#[test]
fn img_archive_of_a_raw_disk_image_or_an_assembly_disk_is_fine() {
    assert!(!archive_warning(
        "[[image.expected_images]]\nname = \"sdcard.img\"\nformat = \"raw\"\nrequired = true\n",
        "",
    ));
    assert!(!archive_warning(
        "[[image.expected_images]]\nname = \"rootfs.ext2\"\nformat = \"ext2\"\nrequired = true\n",
        "[[image.assembly.disks]]\nid = \"sd\"\noutput = \"out/images/sdcard.img\"\n\
         partition_table = \"mbr\"\n\
         [[image.assembly.disks.partitions]]\nname = \"root\"\ntype = \"0x83\"\nimage = \"rootfs.ext2\"\n",
    ));
}
