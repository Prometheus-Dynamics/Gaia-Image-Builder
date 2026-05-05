pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};
use support::write_temp_config;

#[test]
fn buildroot_without_assembly_validates_without_assembly_diagnostics() {
    let path = write_temp_config(
        r#"
build_name = "buildroot-no-assembly-validation"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let report = validate_spec(&spec);

    assert!(
        report
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.code.starts_with("assembly_")),
        "unexpected assembly diagnostic in {report:?}"
    );

    let _ = fs::remove_file(path);
}

#[test]
fn buildroot_expected_image_format_mismatch_is_an_error() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("gaia-buildroot-validate-{nonce}"));
    fs::create_dir_all(root.join("assets")).expect("assets dir");
    let defconfig = root.join("assets/test.defconfig");
    fs::write(&defconfig, "BR2_TARGET_ROOTFS_TAR=y\n").expect("defconfig");
    let config_path = root.join("build.toml");
    fs::write(
        &config_path,
        format!(
            r#"
build_name = "invalid-buildroot-expected-image"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig_path = "{}"

[[image.expected_images]]
name = "rootfs.squashfs"
format = "squashfs"
required = true
"#,
            root.display(),
            defconfig.display()
        ),
    )
    .expect("config");

    let spec = resolve_config(config_path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "buildroot_expected_image_format_not_enabled")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn buildroot_expected_image_can_be_satisfied_by_assembly_disk_output() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("gaia-buildroot-assembly-expected-{nonce}"));
    fs::create_dir_all(root.join("assets")).expect("assets dir");
    let defconfig = root.join("assets/test.defconfig");
    fs::write(&defconfig, "BR2_TARGET_ROOTFS_TAR=y\n").expect("defconfig");
    let config_path = root.join("build.toml");
    fs::write(
        &config_path,
        format!(
            r#"
build_name = "assembly-expected-image"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig_path = "{}"

[[image.expected_images]]
name = "sdcard.img"
format = "raw"
required = true

[image.assembly]
work_dir = "build/assembly"
out_dir = "$provider.images"

[[image.assembly.trees]]
id = "boot"
path = "$assembly.work/boot"

[[image.assembly.disks]]
id = "sdcard"
output = "$provider.images/sdcard.img"
partition_table = "mbr"

[[image.assembly.disks.partitions]]
name = "rootfs"
type_alias = "linux"
image = "$provider.images/rootfs.tar"
"#,
            root.display(),
            defconfig.display()
        ),
    )
    .expect("config");

    let spec = resolve_config(config_path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "buildroot_expected_image_format_not_enabled"),
        "assembly-generated sdcard.img should satisfy expected raw image: {:?}",
        report.diagnostics
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn buildroot_expected_image_still_requires_defconfig_when_not_generated_by_assembly() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("gaia-buildroot-provider-expected-{nonce}"));
    fs::create_dir_all(root.join("assets")).expect("assets dir");
    let defconfig = root.join("assets/test.defconfig");
    fs::write(&defconfig, "BR2_TARGET_ROOTFS_TAR=y\n").expect("defconfig");
    let config_path = root.join("build.toml");
    fs::write(
        &config_path,
        format!(
            r#"
build_name = "provider-expected-image"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig_path = "{}"

[[image.expected_images]]
name = "sdcard.img"
format = "raw"
required = true

[image.assembly]
work_dir = "build/assembly"

[[image.assembly.trees]]
id = "boot"
path = "$assembly.work/boot"
"#,
            root.display(),
            defconfig.display()
        ),
    )
    .expect("config");

    let spec = resolve_config(config_path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "buildroot_expected_image_format_not_enabled"),
        "provider-produced sdcard.img should still require Buildroot raw support"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn image_assembly_validation_reports_invalid_references_and_paths() {
    let path = write_temp_config(
        r#"
build_name = "invalid-assembly"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.assembly]
work_dir = "build/assembly"

[[image.assembly.trees]]
id = "boot"
path = "$assembly.work/boot"

[[image.assembly.trees]]
id = "boot"
path = "$assembly.work/boot2"

[[image.assembly.files]]
tree = "missing"
src = "@assets/config.txt"
src_glob = "@assets/*.txt"
dest = "../config.txt"

[[image.assembly.transforms]]
kind = "gzip"
dest = "$assembly.tree.boot/kernel.img"

[[image.assembly.transforms]]
kind = "copy"
src = "$assembly.tree.missing/kernel.img"
dest = "$provider.sdk/kernel.img"

[[image.assembly.filesystems]]
id = "bootfs"
kind = "vfat"
source_tree = "missing"
output = "$provider.images/boot.vfat"

[[image.assembly.disks]]
id = "sdcard"
output = "$provider.images/sdcard.img"
partition_table = "gpt"
signature = "bad"

[[image.assembly.disks.partitions]]
name = "boot"
type = "0x0C"
type_alias = "fat32-lba"
image = "missing.img"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let report = validate_spec(&spec);
    let codes = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    for expected in [
        "assembly_tree_duplicate",
        "assembly_file_tree_unknown",
        "assembly_file_source_invalid",
        "assembly_file_dest_invalid",
        "assembly_transform_src_required",
        "assembly_path_template_invalid",
        "assembly_filesystem_tree_unknown",
        "assembly_disk_partition_table_unsupported",
        "assembly_disk_signature_raw_invalid",
        "assembly_partition_type_invalid",
        "assembly_partition_image_unknown",
    ] {
        assert!(
            codes.contains(&expected),
            "expected diagnostic {expected}, got {codes:?}"
        );
    }

    let _ = std::fs::remove_file(path);
}

#[test]
fn image_assembly_validation_rejects_unsupported_globs() {
    let path = write_temp_config(
        r#"
build_name = "invalid-assembly-glob"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.assembly]
work_dir = "build/assembly"

[[image.assembly.trees]]
id = "boot"
path = "$assembly.work/boot"

[[image.assembly.files]]
tree = "boot"
src_glob = "@assets/*/*.dtb"
dest = "."

[[image.assembly.files]]
tree = "boot"
src_glob = "@assets/*-*.dtb"
dest = "."
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let report = validate_spec(&spec);
    let unsupported_count = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "assembly_glob_unsupported")
        .count();

    assert_eq!(
        unsupported_count, 2,
        "expected unsupported glob diagnostics, got {report:?}"
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn image_assembly_validation_uses_typed_mode_and_size_parsing() {
    let path = write_temp_config(
        r#"
build_name = "invalid-assembly-typed-values"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.assembly]
work_dir = "build/assembly"

[[image.assembly.trees]]
id = "boot"
path = "$assembly.work/boot"

[[image.assembly.files]]
tree = "boot"
src = "@assets/zero-mode"
dest = "zero-mode"
mode = "0000"

[[image.assembly.files]]
tree = "boot"
src = "@assets/bad-mode"
dest = "bad-mode"
mode = "0888"

[[image.assembly.filesystems]]
id = "bootfs"
kind = "vfat"
source_tree = "boot"
output = "$provider.images/boot.vfat"
size = "not-a-size"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let report = validate_spec(&spec);
    let codes = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(
        codes.contains(&"assembly_file_mode_invalid"),
        "expected invalid mode diagnostic, got {report:?}"
    );
    assert!(
        codes.contains(&"assembly_filesystem_size_invalid"),
        "expected invalid size diagnostic, got {report:?}"
    );
    assert_eq!(
        codes
            .iter()
            .filter(|code| **code == "assembly_file_mode_invalid")
            .count(),
        1,
        "mode 0000 should be accepted while 0888 is rejected: {report:?}"
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn image_assembly_validation_rejects_unsupported_deterministic_settings() {
    let path = write_temp_config(
        r#"
build_name = "invalid-assembly-deterministic"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.assembly]
work_dir = "build/assembly"

[[image.assembly.trees]]
id = "boot"
path = "$assembly.work/boot"

[[image.assembly.transforms]]
kind = "gzip"
src = "@assets/Image"
dest = "$assembly.tree.boot/Image.gz"
deterministic = false

[[image.assembly.filesystems]]
id = "initramfs"
kind = "cpio"
source_tree = "boot"
output = "$provider.images/initramfs.cpio"
deterministic = false

[[image.assembly.filesystems]]
id = "bootfs"
kind = "vfat"
source_tree = "boot"
output = "$provider.images/boot.vfat"
deterministic = true
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let report = validate_spec(&spec);
    let transform_count = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "assembly_transform_deterministic_unsupported")
        .count();
    let filesystem_count = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "assembly_filesystem_deterministic_unsupported")
        .count();

    assert_eq!(
        transform_count, 1,
        "expected transform diagnostic: {report:?}"
    );
    assert_eq!(
        filesystem_count, 2,
        "expected filesystem diagnostics: {report:?}"
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn starting_point_assembly_rejects_unavailable_provider_roots() {
    let path = write_temp_config(
        r#"
build_name = "starting-point-invalid-provider-root"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "starting-point"
rootfs_path = "rootfs"

[image.assembly]
work_dir = "build/assembly"

[[image.assembly.trees]]
id = "host"
path = "$provider.host"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let report = validate_spec(&spec);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "assembly_path_template_invalid")
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn image_assembly_validation_and_roots_reject_tree_self_reference() {
    let path = write_temp_config(
        r#"
build_name = "assembly-tree-self-reference"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "dummy_defconfig"

[image.assembly]
work_dir = "build/assembly"

[[image.assembly.trees]]
id = "boot"
path = "$assembly.tree.boot/nested"
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path should be utf-8"));
    let report = validate_spec(&spec);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "assembly_tree_self_reference"),
        "expected self-reference diagnostic, got {report:?}"
    );
    assert!(
        gaia_spec::AssemblyRoots::new(&spec, spec.image.assembly.as_ref().expect("assembly"))
            .is_err(),
        "runtime roots should reject the same self-referential template"
    );

    let _ = std::fs::remove_file(path);
}
