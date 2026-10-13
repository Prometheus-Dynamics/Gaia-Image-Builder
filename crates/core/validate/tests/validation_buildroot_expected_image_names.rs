use gaia_config::resolve_config;
use gaia_testkit::TempDir;
use gaia_validate::validate_spec;
use std::fs;

#[test]
fn buildroot_expected_image_name_mismatch_is_an_error() {
    let scratch = TempDir::new("gaia-buildroot-name-validate");
    let root = scratch.join("root");
    fs::create_dir_all(root.join("assets")).expect("assets dir");
    let defconfig = root.join("assets/test.defconfig");
    fs::write(&defconfig, "BR2_TARGET_ROOTFS_TAR=y\n").expect("defconfig");
    let config_path = root.join("build.toml");
    fs::write(
        &config_path,
        format!(
            r#"
build_name = "invalid-buildroot-expected-image-name"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig_path = "{}"

[[image.expected_images]]
name = "rootfs.img"
format = "tar"
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
            .any(|diagnostic| diagnostic.code == "buildroot_expected_image_name_mismatch")
    );
}

#[test]
fn buildroot_cpio_expected_image_names_are_accepted() {
    for name in [
        "rootfs.cpio",
        "rootfs.cpio.gz",
        "rootfs.cpio.xz",
        "rootfs.cpio.zst",
        "rootfs.cpio.lz4",
    ] {
        let scratch = TempDir::new("gaia-buildroot-cpio-name");
        let root = scratch.join("root");
        fs::create_dir_all(root.join("assets")).expect("assets dir");
        let defconfig = root.join("assets/test.defconfig");
        fs::write(&defconfig, "BR2_TARGET_ROOTFS_CPIO=y\n").expect("defconfig");
        let config_path = root.join("build.toml");
        fs::write(
            &config_path,
            format!(
                r#"
build_name = "valid-buildroot-cpio-name"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig_path = "{}"

[[image.expected_images]]
name = "{name}"
format = "cpio"
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
            "{name} should be accepted"
        );
    }
}

#[test]
fn buildroot_cpio_expected_image_name_mismatch_is_an_error() {
    let scratch = TempDir::new("gaia-buildroot-cpio-name-invalid");
    let root = scratch.join("root");
    fs::create_dir_all(root.join("assets")).expect("assets dir");
    let defconfig = root.join("assets/test.defconfig");
    fs::write(&defconfig, "BR2_TARGET_ROOTFS_CPIO=y\n").expect("defconfig");
    let config_path = root.join("build.toml");
    fs::write(
        &config_path,
        format!(
            r#"
build_name = "invalid-buildroot-cpio-name"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig_path = "{}"

[[image.expected_images]]
name = "rootfs.tar"
format = "cpio"
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
            .any(|diagnostic| diagnostic.code == "buildroot_expected_image_name_mismatch")
    );
}

#[test]
fn buildroot_cpio_expected_image_requires_defconfig_support() {
    let scratch = TempDir::new("gaia-buildroot-cpio-defconfig");
    let root = scratch.join("root");
    fs::create_dir_all(root.join("assets")).expect("assets dir");
    let defconfig = root.join("assets/test.defconfig");
    fs::write(&defconfig, "BR2_TARGET_ROOTFS_TAR=y\n").expect("defconfig");
    let config_path = root.join("build.toml");
    fs::write(
        &config_path,
        format!(
            r#"
build_name = "invalid-buildroot-cpio-defconfig"

[workspace]
root_dir = "{}"
build_dir = "build"
out_dir = "out"

[image]
kind = "buildroot"
defconfig_path = "{}"

[[image.expected_images]]
name = "rootfs.cpio"
format = "cpio"
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
}

#[test]
fn buildroot_ext2_and_ext3_expected_image_names_are_accepted() {
    for (format_name, image_name, defconfig_contents) in [
        (
            "ext2",
            "rootfs.ext2",
            "BR2_TARGET_ROOTFS_EXT2=y\nBR2_TARGET_ROOTFS_EXT2_2r0=y\n",
        ),
        (
            "ext3",
            "rootfs.ext3",
            "BR2_TARGET_ROOTFS_EXT2=y\nBR2_TARGET_ROOTFS_EXT2_3=y\n",
        ),
    ] {
        let scratch = TempDir::new("gaia-buildroot-name");
        let root = scratch.join("root");
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
    }
}

#[test]
fn buildroot_ext2_and_ext3_expected_image_name_mismatches_are_errors() {
    for (format_name, image_name, defconfig_contents) in [
        (
            "ext2",
            "rootfs.ext3",
            "BR2_TARGET_ROOTFS_EXT2=y\nBR2_TARGET_ROOTFS_EXT2_2r0=y\n",
        ),
        (
            "ext3",
            "rootfs.ext2",
            "BR2_TARGET_ROOTFS_EXT2=y\nBR2_TARGET_ROOTFS_EXT2_3=y\n",
        ),
    ] {
        let scratch = TempDir::new("gaia-buildroot-name-invalid");
        let root = scratch.join("root");
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
    }
}

#[test]
fn buildroot_ext2_and_ext3_expected_images_require_defconfig_support() {
    for (format_name, image_name) in [("ext2", "rootfs.ext2"), ("ext3", "rootfs.ext3")] {
        let scratch = TempDir::new("gaia-buildroot-defconfig");
        let root = scratch.join("root");
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
    }
}
