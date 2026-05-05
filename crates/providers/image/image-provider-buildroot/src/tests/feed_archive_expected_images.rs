use super::*;

#[test]
fn collect_expected_images_reports_missing_required_output() {
    let output_dir = temp_path("gaia-buildroot-collect-expected-missing");
    let collect_dir = temp_path("gaia-buildroot-collect-expected-collect");
    fs::create_dir_all(output_dir.join("images")).expect("images dir");
    fs::create_dir_all(&collect_dir).expect("collect dir");
    let image = ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            expected_images: vec![BuildrootExpectedImageSpec {
                name: "rootfs.tar".into(),
                format: BuildrootExpectedImageFormatSpec::Tar,
                required: true,
            }],
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec {
            collect_dir: None,
            archive_name: None,
            emit_report: true,
        },
        assembly: None,
    };

    let error = collect_expected_images(&image, &output_dir, &collect_dir)
        .expect_err("missing required image should fail");

    assert_eq!(error.kind, ImageProviderErrorKind::OutputMissing);
    assert!(
        error
            .message
            .contains("required buildroot expected image 'rootfs.tar'")
    );
}

#[test]
fn collect_expected_images_skips_required_output_generated_by_assembly() {
    let output_dir = temp_path("gaia-buildroot-collect-assembly-output");
    let collect_dir = temp_path("gaia-buildroot-collect-assembly-collect");
    fs::create_dir_all(output_dir.join("images")).expect("images dir");
    fs::write(output_dir.join("images/rootfs.tar"), "rootfs").expect("provider input");
    fs::create_dir_all(&collect_dir).expect("collect dir");
    let mut image = ImageSpec::new(ImageDefinition::Buildroot(BuildrootImageSpec {
        expected_images: vec![BuildrootExpectedImageSpec {
            name: "sdcard.img".into(),
            format: BuildrootExpectedImageFormatSpec::Raw,
            required: true,
        }],
        ..BuildrootImageSpec::default()
    }));
    image.assembly = Some(ImageAssemblySpec {
        disks: vec![AssemblyDiskSpec {
            id: "sdcard".into(),
            output: "$provider.images/sdcard.img".into(),
            partition_table: AssemblyPartitionTableSpec::Mbr,
            signature: None,
            signature_text: None,
            first_lba: None,
            alignment_lba: None,
            partitions: vec![AssemblyDiskPartitionSpec {
                name: "rootfs".into(),
                kind: Some("0x83".into()),
                type_alias: None,
                bootable: false,
                image: "$provider.images/rootfs.tar".into(),
            }],
        }],
        ..ImageAssemblySpec::default()
    });

    let matched =
        collect_expected_images(&image, &output_dir, &collect_dir).expect("assembly image skipped");

    assert!(matched.is_empty());
    assert!(!collect_dir.join("sdcard.img").exists());
    assert_eq!(
        fs::read_to_string(collect_dir.join("rootfs.tar")).expect("provider input copied"),
        "rootfs"
    );
}

#[test]
fn buildroot_expected_images_present_rejects_assembly_only_outputs_without_provider_inputs() {
    let output_dir = temp_path("gaia-buildroot-present-assembly-output");
    fs::create_dir_all(output_dir.join("images")).expect("images dir");
    let mut image = ImageSpec::new(ImageDefinition::Buildroot(BuildrootImageSpec {
        expected_images: vec![BuildrootExpectedImageSpec {
            name: "sdcard.img".into(),
            format: BuildrootExpectedImageFormatSpec::Raw,
            required: true,
        }],
        ..BuildrootImageSpec::default()
    }));
    image.assembly = Some(ImageAssemblySpec {
        disks: vec![AssemblyDiskSpec {
            id: "sdcard".into(),
            output: "$provider.images/sdcard.img".into(),
            partition_table: AssemblyPartitionTableSpec::Mbr,
            signature: None,
            signature_text: None,
            first_lba: None,
            alignment_lba: None,
            partitions: Vec::new(),
        }],
        ..ImageAssemblySpec::default()
    });

    assert!(!buildroot_expected_images_present(&image, &output_dir));
}

#[test]
fn buildroot_expected_images_present_requires_provider_root_assembly_inputs() {
    let collect_dir = temp_path("gaia-buildroot-present-provider-input-collect");
    let output_dir = collect_dir.join("buildroot-output");
    fs::create_dir_all(output_dir.join("images")).expect("images dir");
    let mut image = ImageSpec::new(ImageDefinition::Buildroot(BuildrootImageSpec {
        expected_images: vec![BuildrootExpectedImageSpec {
            name: "sdcard.img".into(),
            format: BuildrootExpectedImageFormatSpec::Raw,
            required: true,
        }],
        ..BuildrootImageSpec::default()
    }));
    image.assembly = Some(ImageAssemblySpec {
        disks: vec![AssemblyDiskSpec {
            id: "sdcard".into(),
            output: "$provider.images/sdcard.img".into(),
            partition_table: AssemblyPartitionTableSpec::Mbr,
            signature: None,
            signature_text: None,
            first_lba: None,
            alignment_lba: None,
            partitions: vec![AssemblyDiskPartitionSpec {
                name: "rootfs".into(),
                kind: Some("0x83".into()),
                type_alias: None,
                bootable: false,
                image: "$provider.images/rootfs.tar".into(),
            }],
        }],
        ..ImageAssemblySpec::default()
    });

    assert!(!buildroot_expected_images_present(&image, &output_dir));

    fs::write(output_dir.join("images/rootfs.tar"), "rootfs").expect("provider input");
    assert!(buildroot_expected_images_present(&image, &output_dir));

    fs::remove_file(output_dir.join("images/rootfs.tar")).expect("remove provider input");
    fs::write(collect_dir.join("rootfs.tar"), "rootfs").expect("collected provider input");
    assert!(buildroot_expected_images_present(&image, &output_dir));
}

#[test]
fn collect_expected_images_copies_board_style_raw_image() {
    let output_dir = temp_path("gaia-buildroot-collect-raw-output");
    let collect_dir = temp_path("gaia-buildroot-collect-raw-collect");
    fs::create_dir_all(output_dir.join("images")).expect("images dir");
    fs::write(output_dir.join("images/sdcard.img"), "disk-image").expect("sdcard image");
    let image = ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            expected_images: vec![BuildrootExpectedImageSpec {
                name: "sdcard.img".into(),
                format: BuildrootExpectedImageFormatSpec::Raw,
                required: true,
            }],
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec {
            collect_dir: None,
            archive_name: None,
            emit_report: true,
        },
        assembly: None,
    };

    let matched =
        collect_expected_images(&image, &output_dir, &collect_dir).expect("raw image collect");

    assert_eq!(matched, vec!["sdcard.img".to_string()]);
    assert_eq!(
        fs::read_to_string(collect_dir.join("sdcard.img")).expect("collected raw image"),
        "disk-image"
    );
}

#[test]
fn collect_expected_images_copies_raw_board_image_into_collect_dir() {
    let output_dir = temp_path("gaia-buildroot-collect-raw-output");
    let collect_dir = temp_path("gaia-buildroot-collect-raw-collect");
    let images_dir = output_dir.join("images");
    fs::create_dir_all(&images_dir).expect("images dir");
    fs::create_dir_all(&collect_dir).expect("collect dir");
    fs::write(images_dir.join("sdcard.img"), "raw-image").expect("raw image");
    let image = ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            expected_images: vec![BuildrootExpectedImageSpec {
                name: "sdcard.img".into(),
                format: BuildrootExpectedImageFormatSpec::Raw,
                required: true,
            }],
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec {
            collect_dir: None,
            archive_name: None,
            emit_report: true,
        },
        assembly: None,
    };

    let matched = collect_expected_images(&image, &output_dir, &collect_dir)
        .expect("raw board image should collect");

    assert_eq!(matched, vec!["sdcard.img".to_string()]);
    assert_eq!(
        fs::read_to_string(collect_dir.join("sdcard.img")).expect("collected image"),
        "raw-image"
    );
}

#[test]
fn collect_expected_images_copies_required_cpio_image() {
    let output_dir = temp_path("gaia-buildroot-collect-cpio-output");
    let collect_dir = temp_path("gaia-buildroot-collect-cpio-collect");
    fs::create_dir_all(output_dir.join("images")).expect("images dir");
    fs::write(output_dir.join("images/rootfs.cpio.gz"), "cpio-image").expect("cpio image");
    let image = ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            expected_images: vec![BuildrootExpectedImageSpec {
                name: "rootfs.cpio.gz".into(),
                format: BuildrootExpectedImageFormatSpec::Cpio,
                required: true,
            }],
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec {
            collect_dir: None,
            archive_name: None,
            emit_report: true,
        },
        assembly: None,
    };

    let matched =
        collect_expected_images(&image, &output_dir, &collect_dir).expect("cpio image collect");

    assert_eq!(matched, vec!["rootfs.cpio.gz".to_string()]);
    assert_eq!(
        fs::read_to_string(collect_dir.join("rootfs.cpio.gz")).expect("collected cpio image"),
        "cpio-image"
    );
}

#[test]
fn collect_expected_images_skips_missing_optional_cpio_image() {
    let output_dir = temp_path("gaia-buildroot-collect-optional-cpio-output");
    let collect_dir = temp_path("gaia-buildroot-collect-optional-cpio-collect");
    fs::create_dir_all(output_dir.join("images")).expect("images dir");
    let image = ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            expected_images: vec![BuildrootExpectedImageSpec {
                name: "rootfs.cpio".into(),
                format: BuildrootExpectedImageFormatSpec::Cpio,
                required: false,
            }],
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec {
            collect_dir: None,
            archive_name: None,
            emit_report: true,
        },
        assembly: None,
    };

    let matched = collect_expected_images(&image, &output_dir, &collect_dir)
        .expect("optional missing cpio image should not fail");

    assert!(matched.is_empty());
}

#[test]
fn collect_expected_images_copies_ext2_and_ext3_images() {
    for (format, name, contents) in [
        (
            BuildrootExpectedImageFormatSpec::Ext2,
            "rootfs.ext2",
            "ext2-image",
        ),
        (
            BuildrootExpectedImageFormatSpec::Ext3,
            "rootfs.ext3",
            "ext3-image",
        ),
    ] {
        let output_dir = temp_path(&format!("gaia-buildroot-collect-{name}-output"));
        let collect_dir = temp_path(&format!("gaia-buildroot-collect-{name}-collect"));
        fs::create_dir_all(output_dir.join("images")).expect("images dir");
        fs::write(output_dir.join("images").join(name), contents).expect("ext image");
        let image = ImageSpec {
            definition: ImageDefinition::Buildroot(BuildrootImageSpec {
                expected_images: vec![BuildrootExpectedImageSpec {
                    name: name.into(),
                    format,
                    required: true,
                }],
                ..BuildrootImageSpec::default()
            }),
            feed: gaia_spec::ImageFeedSpec::default(),
            output: ImageOutputSpec {
                collect_dir: None,
                archive_name: None,
                emit_report: true,
            },
            assembly: None,
        };

        let matched =
            collect_expected_images(&image, &output_dir, &collect_dir).expect("ext image collect");

        assert_eq!(matched, vec![name.to_string()]);
        assert_eq!(
            fs::read_to_string(collect_dir.join(name)).expect("collected ext image"),
            contents
        );
    }
}

#[test]
fn collect_expected_images_skips_missing_optional_ext2_and_ext3_images() {
    for (format, name) in [
        (BuildrootExpectedImageFormatSpec::Ext2, "rootfs.ext2"),
        (BuildrootExpectedImageFormatSpec::Ext3, "rootfs.ext3"),
    ] {
        let output_dir = temp_path(&format!("gaia-buildroot-collect-optional-{name}-output"));
        let collect_dir = temp_path(&format!("gaia-buildroot-collect-optional-{name}-collect"));
        fs::create_dir_all(output_dir.join("images")).expect("images dir");
        let image = ImageSpec {
            definition: ImageDefinition::Buildroot(BuildrootImageSpec {
                expected_images: vec![BuildrootExpectedImageSpec {
                    name: name.into(),
                    format,
                    required: false,
                }],
                ..BuildrootImageSpec::default()
            }),
            feed: gaia_spec::ImageFeedSpec::default(),
            output: ImageOutputSpec {
                collect_dir: None,
                archive_name: None,
                emit_report: true,
            },
            assembly: None,
        };

        let matched = collect_expected_images(&image, &output_dir, &collect_dir)
            .expect("optional missing ext image should not fail");

        assert!(matched.is_empty());
    }
}

#[test]
fn collect_expected_images_copies_flash_and_erofs_images() {
    for (format, name, contents) in [
        (
            BuildrootExpectedImageFormatSpec::Ubifs,
            "rootfs.ubifs",
            "ubifs-image",
        ),
        (
            BuildrootExpectedImageFormatSpec::Ubi,
            "rootfs.ubi",
            "ubi-image",
        ),
        (
            BuildrootExpectedImageFormatSpec::Jffs2,
            "rootfs.jffs2",
            "jffs2-image",
        ),
        (
            BuildrootExpectedImageFormatSpec::Erofs,
            "rootfs.erofs",
            "erofs-image",
        ),
    ] {
        let output_dir = temp_path(&format!("gaia-buildroot-collect-{name}-output"));
        let collect_dir = temp_path(&format!("gaia-buildroot-collect-{name}-collect"));
        fs::create_dir_all(output_dir.join("images")).expect("images dir");
        fs::write(output_dir.join("images").join(name), contents).expect("image");
        let image = ImageSpec {
            definition: ImageDefinition::Buildroot(BuildrootImageSpec {
                expected_images: vec![BuildrootExpectedImageSpec {
                    name: name.into(),
                    format,
                    required: true,
                }],
                ..BuildrootImageSpec::default()
            }),
            feed: gaia_spec::ImageFeedSpec::default(),
            output: ImageOutputSpec {
                collect_dir: None,
                archive_name: None,
                emit_report: true,
            },
            assembly: None,
        };

        let matched =
            collect_expected_images(&image, &output_dir, &collect_dir).expect("image collect");

        assert_eq!(matched, vec![name.to_string()]);
        assert_eq!(
            fs::read_to_string(collect_dir.join(name)).expect("collected image"),
            contents
        );
    }
}

#[test]
fn collect_expected_images_skips_missing_optional_flash_and_erofs_images() {
    for (format, name) in [
        (BuildrootExpectedImageFormatSpec::Ubifs, "rootfs.ubifs"),
        (BuildrootExpectedImageFormatSpec::Ubi, "rootfs.ubi"),
        (BuildrootExpectedImageFormatSpec::Jffs2, "rootfs.jffs2"),
        (BuildrootExpectedImageFormatSpec::Erofs, "rootfs.erofs"),
    ] {
        let output_dir = temp_path(&format!("gaia-buildroot-collect-optional-{name}-output"));
        let collect_dir = temp_path(&format!("gaia-buildroot-collect-optional-{name}-collect"));
        fs::create_dir_all(output_dir.join("images")).expect("images dir");
        let image = ImageSpec {
            definition: ImageDefinition::Buildroot(BuildrootImageSpec {
                expected_images: vec![BuildrootExpectedImageSpec {
                    name: name.into(),
                    format,
                    required: false,
                }],
                ..BuildrootImageSpec::default()
            }),
            feed: gaia_spec::ImageFeedSpec::default(),
            output: ImageOutputSpec {
                collect_dir: None,
                archive_name: None,
                emit_report: true,
            },
            assembly: None,
        };

        let matched = collect_expected_images(&image, &output_dir, &collect_dir)
            .expect("optional missing image should not fail");

        assert!(matched.is_empty());
    }
}

#[test]
fn collect_expected_images_copies_lower_priority_images() {
    for (format, name, contents) in [
        (
            BuildrootExpectedImageFormatSpec::Romfs,
            "rootfs.romfs",
            "romfs-image",
        ),
        (
            BuildrootExpectedImageFormatSpec::Cramfs,
            "rootfs.cramfs",
            "cramfs-image",
        ),
        (
            BuildrootExpectedImageFormatSpec::Cloop,
            "rootfs.cloop",
            "cloop-image",
        ),
        (
            BuildrootExpectedImageFormatSpec::F2fs,
            "rootfs.f2fs",
            "f2fs-image",
        ),
        (
            BuildrootExpectedImageFormatSpec::Btrfs,
            "rootfs.btrfs",
            "btrfs-image",
        ),
    ] {
        let output_dir = temp_path(&format!("gaia-buildroot-collect-{name}-output"));
        let collect_dir = temp_path(&format!("gaia-buildroot-collect-{name}-collect"));
        fs::create_dir_all(output_dir.join("images")).expect("images dir");
        fs::write(output_dir.join("images").join(name), contents).expect("image");
        let image = ImageSpec {
            definition: ImageDefinition::Buildroot(BuildrootImageSpec {
                expected_images: vec![BuildrootExpectedImageSpec {
                    name: name.into(),
                    format,
                    required: true,
                }],
                ..BuildrootImageSpec::default()
            }),
            feed: gaia_spec::ImageFeedSpec::default(),
            output: ImageOutputSpec {
                collect_dir: None,
                archive_name: None,
                emit_report: true,
            },
            assembly: None,
        };

        let matched =
            collect_expected_images(&image, &output_dir, &collect_dir).expect("image collect");

        assert_eq!(matched, vec![name.to_string()]);
        assert_eq!(
            fs::read_to_string(collect_dir.join(name)).expect("collected image"),
            contents
        );
    }
}

#[test]
fn collect_expected_images_skips_missing_optional_lower_priority_images() {
    for (format, name) in [
        (BuildrootExpectedImageFormatSpec::Romfs, "rootfs.romfs"),
        (BuildrootExpectedImageFormatSpec::Cramfs, "rootfs.cramfs"),
        (BuildrootExpectedImageFormatSpec::Cloop, "rootfs.cloop"),
        (BuildrootExpectedImageFormatSpec::F2fs, "rootfs.f2fs"),
        (BuildrootExpectedImageFormatSpec::Btrfs, "rootfs.btrfs"),
    ] {
        let output_dir = temp_path(&format!("gaia-buildroot-collect-optional-{name}-output"));
        let collect_dir = temp_path(&format!("gaia-buildroot-collect-optional-{name}-collect"));
        fs::create_dir_all(output_dir.join("images")).expect("images dir");
        let image = ImageSpec {
            definition: ImageDefinition::Buildroot(BuildrootImageSpec {
                expected_images: vec![BuildrootExpectedImageSpec {
                    name: name.into(),
                    format,
                    required: false,
                }],
                ..BuildrootImageSpec::default()
            }),
            feed: gaia_spec::ImageFeedSpec::default(),
            output: ImageOutputSpec {
                collect_dir: None,
                archive_name: None,
                emit_report: true,
            },
            assembly: None,
        };

        let matched = collect_expected_images(&image, &output_dir, &collect_dir)
            .expect("optional missing image should not fail");

        assert!(matched.is_empty());
    }
}
