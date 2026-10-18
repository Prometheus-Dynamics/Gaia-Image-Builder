//! `image.output.disk` (the assembly disk a raw disk archive is built from)
//! and the xz transform's level range.

pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use support::write_temp_config;

/// Two assembly disks (the second one in the work dir), an xz transform and
/// the output section given by the caller.
fn recipe(output: &str, transform: &str) -> String {
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

[image.output]
collect_dir = "out/images"
{output}

[image.assembly]
work_dir = "${{workspace.build_dir}}/assembly"

[[image.assembly.disks]]
id = "sdcard"
output = "$provider.images/sdcard.img"
partition_table = "mbr"

[[image.assembly.disks.partitions]]
name = "rootfs"
type_alias = "linux"
size = "16M"

[[image.assembly.disks]]
id = "slots-sdcard"
output = "$assembly.work/slots/slots-sdcard.img"
partition_table = "mbr"

[[image.assembly.disks.partitions]]
name = "data"
type_alias = "linux"
size = "16M"

{transform}
"#
    )
}

fn output_disk_codes(contents: &str) -> Vec<&'static str> {
    let path = write_temp_config(contents);
    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    validate_spec(&spec)
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code.starts_with("image_output_disk")
                || diagnostic.code.starts_with("assembly_transform_level")
        })
        .map(|diagnostic| diagnostic.code)
        .collect()
}

const XZ: &str = r#"
[[image.assembly.transforms]]
kind = "xz"
src = "$assembly.work/slots/slots-sdcard.img"
dest = "$provider.images/slots-sdcard.img.xz"
level = 9
"#;

#[test]
fn output_disk_naming_an_assembly_disk_with_a_disk_archive_validates() {
    let contents = recipe(
        r#"archive_name = "acme.img.xz"
disk = "sdcard""#,
        XZ,
    );
    assert_eq!(output_disk_codes(&contents), Vec::<&str>::new());
}

#[test]
fn output_disk_must_name_an_assembly_disk() {
    let contents = recipe(
        r#"archive_name = "acme.img.xz"
disk = "emmc""#,
        XZ,
    );
    assert_eq!(
        output_disk_codes(&contents),
        vec!["image_output_disk_unknown"]
    );
}

#[test]
fn output_disk_needs_a_raw_disk_archive_name() {
    let contents = recipe(
        r#"archive_name = "acme.tar.xz"
disk = "sdcard""#,
        XZ,
    );
    assert_eq!(
        output_disk_codes(&contents),
        vec!["image_output_disk_without_disk_archive"]
    );
}

#[test]
fn xz_levels_are_zero_to_nine() {
    let ok = recipe(r#"archive_name = "acme.img.xz""#, XZ);
    assert_eq!(output_disk_codes(&ok), Vec::<&str>::new());
    let too_high = recipe(
        r#"archive_name = "acme.img.xz""#,
        &XZ.replace("level = 9", "level = 10"),
    );
    assert_eq!(
        output_disk_codes(&too_high),
        vec!["assembly_transform_level_invalid"]
    );
}

#[test]
fn output_disk_and_xz_transform_resolve_from_the_config() {
    let contents = recipe(
        r#"archive_name = "acme.img.xz"
disk = "sdcard""#,
        XZ,
    );
    let path = write_temp_config(&contents);
    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    assert_eq!(spec.image.output.disk.as_deref(), Some("sdcard"));
    let assembly = spec.image.assembly.as_ref().expect("assembly");
    assert_eq!(
        assembly.transforms[0].kind,
        gaia_spec::AssemblyTransformKindSpec::Xz
    );
    assert_eq!(assembly.transforms[0].level, Some(9));
}
