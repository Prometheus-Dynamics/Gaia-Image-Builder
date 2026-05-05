pub mod support;

use gaia_config::resolve_config;
use gaia_validate::validate_spec;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};
use support::write_temp_config;

#[test]
fn buildroot_flash_and_erofs_expected_image_names_are_accepted() {
    for (format_name, image_name, defconfig_contents) in [
        ("ubifs", "rootfs.ubifs", "BR2_TARGET_ROOTFS_UBIFS=y\n"),
        ("ubi", "rootfs.ubi", "BR2_TARGET_ROOTFS_UBI=y\n"),
        ("jffs2", "rootfs.jffs2", "BR2_TARGET_ROOTFS_JFFS2=y\n"),
        ("erofs", "rootfs.erofs", "BR2_TARGET_ROOTFS_EROFS=y\n"),
    ] {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("gaia-buildroot-{format_name}-name-{nonce}"));
        fs::create_dir_all(root.join("assets")).expect("assets dir");
        let defconfig = root.join("assets/test.defconfig");
        fs::write(&defconfig, defconfig_contents).expect("defconfig");
        let config_path = root.join("build.toml");
        fs::write(
            &config_path,
            format!(
                r#"
build_name = "valid-buildroot-{format_name}-name"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig_path = "{}"

[[image.expected_images]]
name = "{image_name}"
format = "{format_name}"
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
            !report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "buildroot_expected_image_name_mismatch"),
            "{image_name} should be accepted"
        );
        assert!(
            !report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "buildroot_expected_image_format_not_enabled"),
            "{format_name} defconfig should be accepted"
        );

        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn buildroot_flash_and_erofs_expected_image_name_mismatches_are_errors() {
    for (format_name, image_name, defconfig_contents) in [
        ("ubifs", "rootfs.ubi", "BR2_TARGET_ROOTFS_UBIFS=y\n"),
        ("ubi", "rootfs.ubifs", "BR2_TARGET_ROOTFS_UBI=y\n"),
        ("jffs2", "rootfs.ubi", "BR2_TARGET_ROOTFS_JFFS2=y\n"),
        ("erofs", "rootfs.ubi", "BR2_TARGET_ROOTFS_EROFS=y\n"),
    ] {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("gaia-buildroot-{format_name}-name-invalid-{nonce}"));
        fs::create_dir_all(root.join("assets")).expect("assets dir");
        let defconfig = root.join("assets/test.defconfig");
        fs::write(&defconfig, defconfig_contents).expect("defconfig");
        let config_path = root.join("build.toml");
        fs::write(
            &config_path,
            format!(
                r#"
build_name = "invalid-buildroot-{format_name}-name"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig_path = "{}"

[[image.expected_images]]
name = "{image_name}"
format = "{format_name}"
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
                .any(|diagnostic| diagnostic.code == "buildroot_expected_image_name_mismatch"),
            "{image_name} should be rejected for {format_name}"
        );

        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn buildroot_flash_and_erofs_expected_images_require_defconfig_support() {
    for (format_name, image_name) in [
        ("ubifs", "rootfs.ubifs"),
        ("ubi", "rootfs.ubi"),
        ("jffs2", "rootfs.jffs2"),
        ("erofs", "rootfs.erofs"),
    ] {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("gaia-buildroot-{format_name}-defconfig-{nonce}"));
        fs::create_dir_all(root.join("assets")).expect("assets dir");
        let defconfig = root.join("assets/test.defconfig");
        fs::write(&defconfig, "BR2_TARGET_ROOTFS_TAR=y\n").expect("defconfig");
        let config_path = root.join("build.toml");
        fs::write(
            &config_path,
            format!(
                r#"
build_name = "invalid-buildroot-{format_name}-defconfig"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig_path = "{}"

[[image.expected_images]]
name = "{image_name}"
format = "{format_name}"
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
                .any(|diagnostic| diagnostic.code == "buildroot_expected_image_format_not_enabled"),
            "{format_name} should require defconfig support"
        );

        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn buildroot_lower_priority_expected_image_names_are_accepted() {
    for (format_name, image_name, defconfig_contents) in [
        ("romfs", "rootfs.romfs", "BR2_TARGET_ROOTFS_ROMFS=y\n"),
        ("cramfs", "rootfs.cramfs", "BR2_TARGET_ROOTFS_CRAMFS=y\n"),
        ("cloop", "rootfs.cloop", "BR2_TARGET_ROOTFS_CLOOP=y\n"),
        ("f2fs", "rootfs.f2fs", "BR2_TARGET_ROOTFS_F2FS=y\n"),
        ("btrfs", "rootfs.btrfs", "BR2_TARGET_ROOTFS_BTRFS=y\n"),
    ] {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("gaia-buildroot-{format_name}-name-{nonce}"));
        fs::create_dir_all(root.join("assets")).expect("assets dir");
        let defconfig = root.join("assets/test.defconfig");
        fs::write(&defconfig, defconfig_contents).expect("defconfig");
        let config_path = root.join("build.toml");
        fs::write(
            &config_path,
            format!(
                r#"
build_name = "valid-buildroot-{format_name}-name"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig_path = "{}"

[[image.expected_images]]
name = "{image_name}"
format = "{format_name}"
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
            !report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "buildroot_expected_image_name_mismatch"),
            "{image_name} should be accepted"
        );
        assert!(
            !report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "buildroot_expected_image_format_not_enabled"),
            "{format_name} defconfig should be accepted"
        );

        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn buildroot_lower_priority_expected_image_name_mismatches_are_errors() {
    for (format_name, image_name, defconfig_contents) in [
        ("romfs", "rootfs.cramfs", "BR2_TARGET_ROOTFS_ROMFS=y\n"),
        ("cramfs", "rootfs.romfs", "BR2_TARGET_ROOTFS_CRAMFS=y\n"),
        ("cloop", "rootfs.romfs", "BR2_TARGET_ROOTFS_CLOOP=y\n"),
        ("f2fs", "rootfs.romfs", "BR2_TARGET_ROOTFS_F2FS=y\n"),
        ("btrfs", "rootfs.romfs", "BR2_TARGET_ROOTFS_BTRFS=y\n"),
    ] {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("gaia-buildroot-{format_name}-name-invalid-{nonce}"));
        fs::create_dir_all(root.join("assets")).expect("assets dir");
        let defconfig = root.join("assets/test.defconfig");
        fs::write(&defconfig, defconfig_contents).expect("defconfig");
        let config_path = root.join("build.toml");
        fs::write(
            &config_path,
            format!(
                r#"
build_name = "invalid-buildroot-{format_name}-name"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig_path = "{}"

[[image.expected_images]]
name = "{image_name}"
format = "{format_name}"
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
                .any(|diagnostic| diagnostic.code == "buildroot_expected_image_name_mismatch"),
            "{image_name} should be rejected for {format_name}"
        );

        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn buildroot_lower_priority_expected_images_require_defconfig_support() {
    for (format_name, image_name) in [
        ("romfs", "rootfs.romfs"),
        ("cramfs", "rootfs.cramfs"),
        ("cloop", "rootfs.cloop"),
        ("f2fs", "rootfs.f2fs"),
        ("btrfs", "rootfs.btrfs"),
    ] {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("gaia-buildroot-{format_name}-defconfig-{nonce}"));
        fs::create_dir_all(root.join("assets")).expect("assets dir");
        let defconfig = root.join("assets/test.defconfig");
        fs::write(&defconfig, "BR2_TARGET_ROOTFS_TAR=y\n").expect("defconfig");
        let config_path = root.join("build.toml");
        fs::write(
            &config_path,
            format!(
                r#"
build_name = "invalid-buildroot-{format_name}-defconfig"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig_path = "{}"

[[image.expected_images]]
name = "{image_name}"
format = "{format_name}"
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
                .any(|diagnostic| diagnostic.code == "buildroot_expected_image_format_not_enabled"),
            "{format_name} should require defconfig support"
        );

        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn buildroot_config_fragments_require_base_config() {
    let path = write_temp_config(
        r#"
build_name = "invalid-buildroot-fragments"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
config_fragments = ["assets/fragment.cfg"]
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "buildroot_config_fragments_require_base_config")
    );

    let _ = fs::remove_file(path);
}

#[test]
fn missing_buildroot_config_fragment_is_an_error() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("gaia-buildroot-fragment-validate-{nonce}"));
    fs::create_dir_all(root.join("assets")).expect("assets dir");
    let defconfig = root.join("assets/test.defconfig");
    fs::write(&defconfig, "BR2_TARGET_ROOTFS_TAR=y\n").expect("defconfig");
    let config_path = root.join("build.toml");
    fs::write(
        &config_path,
        format!(
            r#"
build_name = "missing-buildroot-fragment"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig_path = "{}"
config_fragments = ["assets/missing-fragment.cfg"]
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
            .any(|diagnostic| diagnostic.code == "buildroot_config_fragment_missing")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn buildroot_config_overrides_require_base_config() {
    let path = write_temp_config(
        r#"
build_name = "invalid-buildroot-overrides"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
config_overrides = [["BR2_PACKAGE_BUSYBOX", "y"]]
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "buildroot_config_overrides_require_base_config")
    );

    let _ = fs::remove_file(path);
}

#[test]
fn invalid_buildroot_config_override_key_is_an_error() {
    let path = write_temp_config(
        r#"
build_name = "invalid-buildroot-override-key"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "qemu_x86_64_defconfig"
config_overrides = [["HOSTNAME", "\"bad\""]]
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "buildroot_config_override_key_invalid")
    );

    let _ = fs::remove_file(path);
}

#[test]
fn invalid_image_contract_fields_are_rejected() {
    let path = write_temp_config(
        r#"
build_name = "invalid-image-contract"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig = "rpi_defconfig"
external_tree = "external"
external_tree_mode = "disabled"

[image.feed]
install_entries = ["missing-install"]
stage_files = ["missing-stage-file"]

[[image.expected_images]]
name = ""
format = "tar"
required = true
"#,
    );

    let spec = resolve_config(path.to_str().expect("temp path utf-8"));
    let report = validate_spec(&spec);

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "buildroot_external_tree_disabled")
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "buildroot_expected_image_empty")
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "unknown_image_feed_install")
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "unknown_image_feed_stage_file")
    );

    let _ = fs::remove_file(path);
}
