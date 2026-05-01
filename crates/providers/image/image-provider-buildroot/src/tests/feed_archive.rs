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
fn archive_buildroot_output_promotes_single_primary_image() {
    let collect_dir = temp_path("gaia-buildroot-collect-dir");
    let output_dir = temp_path("gaia-buildroot-output-dir");
    let archive_path = temp_path("gaia-buildroot-primary-archive");
    fs::create_dir_all(&collect_dir).expect("collect dir");
    fs::create_dir_all(&output_dir).expect("output dir");
    fs::write(collect_dir.join("rootfs.squashfs"), "primary image").expect("image");
    let image = ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            expected_images: vec![BuildrootExpectedImageSpec {
                name: "rootfs.squashfs".into(),
                format: BuildrootExpectedImageFormatSpec::Squashfs,
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
    };

    let matched = vec!["rootfs.squashfs".to_string()];
    let mut reuse_details = Vec::new();
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();
    archive_buildroot_output(BuildrootArchiveRequest {
        image: &image,
        collect_dir: &collect_dir,
        output_dir: &output_dir,
        matched_expected_images: &matched,
        archive_path: &archive_path,
        reuse_details: &mut reuse_details,
        command: test_command_context(&execution, &policy),
    })
    .expect("primary image promotion should succeed");

    assert_eq!(
        fs::read_to_string(&archive_path).expect("archive contents"),
        "primary image"
    );
}

#[test]
fn collect_expected_images_and_archive_promote_raw_board_image() {
    let output_dir = temp_path("gaia-buildroot-raw-board-out");
    let collect_dir = temp_path("gaia-buildroot-raw-board-collect");
    let archive_path = temp_path("gaia-buildroot-raw-board-archive.img");
    fs::create_dir_all(output_dir.join("images")).expect("images dir");
    fs::write(output_dir.join("images/sdcard.img"), "raw board image").expect("raw image");
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
    };

    let matched = collect_expected_images(&image, &output_dir, &collect_dir)
        .expect("raw image should be collected");
    assert_eq!(matched, vec!["sdcard.img".to_string()]);
    assert_eq!(
        fs::read_to_string(collect_dir.join("sdcard.img")).expect("collected raw image"),
        "raw board image"
    );

    let mut reuse_details = Vec::new();
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();
    archive_buildroot_output(BuildrootArchiveRequest {
        image: &image,
        collect_dir: &collect_dir,
        output_dir: &output_dir,
        matched_expected_images: &matched,
        archive_path: &archive_path,
        reuse_details: &mut reuse_details,
        command: test_command_context(&execution, &policy),
    })
    .expect("raw image should be promoted");

    assert_eq!(
        fs::read_to_string(&archive_path).expect("promoted raw image"),
        "raw board image"
    );
}

#[test]
fn image_feed_has_content_is_false_for_bare_image() {
    let image = ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec::default()),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec {
            collect_dir: None,
            archive_name: None,
            emit_report: true,
        },
    };

    assert!(!image_feed_has_content(&image));
}

#[test]
fn image_feed_has_content_is_true_when_stage_or_install_entries_exist() {
    let image = ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec::default()),
        feed: gaia_spec::ImageFeedSpec {
            install_entries: vec!["install-app".into()],
            stage_files: vec!["motd".into()],
            stage_env_sets: vec![],
            stage_services: vec![],
        },
        output: ImageOutputSpec {
            collect_dir: None,
            archive_name: None,
            emit_report: true,
        },
    };

    assert!(image_feed_has_content(&image));
}

#[test]
fn archive_buildroot_output_archives_final_raw_image_into_compressed_tar() {
    let collect_dir = temp_path("gaia-buildroot-collect-multi-dir");
    let output_dir = temp_path("gaia-buildroot-output-multi-dir");
    let archive_path = collect_dir.join("bundle.tar.xz");
    fs::create_dir_all(&collect_dir).expect("collect dir");
    fs::create_dir_all(&output_dir).expect("output dir");
    fs::write(collect_dir.join("rootfs.squashfs"), "squashfs image").expect("rootfs squashfs");
    fs::write(collect_dir.join("sdcard.img"), "raw image").expect("raw image");
    fs::write(collect_dir.join("ignored.txt"), "not expected").expect("ignored");
    let image = ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            expected_images: vec![
                BuildrootExpectedImageSpec {
                    name: "rootfs.squashfs".into(),
                    format: BuildrootExpectedImageFormatSpec::Squashfs,
                    required: true,
                },
                BuildrootExpectedImageSpec {
                    name: "sdcard.img".into(),
                    format: BuildrootExpectedImageFormatSpec::Raw,
                    required: true,
                },
            ],
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec {
            collect_dir: None,
            archive_name: None,
            emit_report: true,
        },
    };

    let matched = vec!["rootfs.squashfs".to_string(), "sdcard.img".to_string()];
    let mut reuse_details = Vec::new();
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();
    archive_buildroot_output(BuildrootArchiveRequest {
        image: &image,
        collect_dir: &collect_dir,
        output_dir: &output_dir,
        matched_expected_images: &matched,
        archive_path: &archive_path,
        reuse_details: &mut reuse_details,
        command: test_command_context(&execution, &policy),
    })
    .expect("directory archive should succeed");

    let listing = Command::new("tar")
        .arg("-tf")
        .arg(&archive_path)
        .output()
        .expect("tar listing");
    assert!(
        listing.status.success(),
        "expected archive listing to succeed"
    );
    let listing = String::from_utf8_lossy(&listing.stdout);
    assert!(listing.contains("sdcard.img"));
    assert!(!listing.contains("rootfs.squashfs"));
    assert!(!listing.contains("bundle.tar"));
    assert!(!listing.contains("ignored.txt"));

    let messages = archive_buildroot_output(BuildrootArchiveRequest {
        image: &image,
        collect_dir: &collect_dir,
        output_dir: &output_dir,
        matched_expected_images: &matched,
        archive_path: &archive_path,
        reuse_details: &mut reuse_details,
        command: test_command_context(&execution, &policy),
    })
    .expect("unchanged archive should be reused");
    assert_eq!(reuse_details, vec!["image-archive".to_string()]);
    assert!(
        messages
            .iter()
            .any(|message| { message.contains("reused image archive") })
    );
}

#[test]
fn apply_image_feed_to_rootfs_writes_install_file_env_and_service_content() {
    let workspace_root = temp_path("gaia-buildroot-feed-workspace");
    let rootfs_dir = temp_path("gaia-buildroot-feed-rootfs");
    let artifact_dir = workspace_root.join("out/artifacts");
    let assets_dir = workspace_root.join("assets");
    fs::create_dir_all(&artifact_dir).expect("artifact dir");
    fs::create_dir_all(&assets_dir).expect("assets dir");

    let artifact_path = artifact_dir.join("smoke-app");
    let stage_file_path = assets_dir.join("motd");
    let service_path = assets_dir.join("gaia-smoke.service");
    fs::write(&artifact_path, "binary").expect("artifact file");
    fs::write(&stage_file_path, "hello motd").expect("stage file");
    fs::write(&service_path, "[Service]\nExecStart=/usr/bin/smoke-app\n").expect("service");

    let mut spec = ResolvedBuildSpec::new("buildroot-feed-test");
    spec.workspace.root_dir = workspace_root.display().to_string();
    spec.artifacts.push(gaia_spec::ArtifactSpec::new(
        "smoke-app",
        gaia_spec::ArtifactDefinition::Rust(gaia_spec::RustArtifactSpec {
            package: "smoke-app".into(),
            target_name: None,
            variant: gaia_spec::ArtifactVariantSpec::File,
        }),
        None,
        gaia_spec::ArtifactOutputSpec {
            path: artifact_path.display().to_string(),
        },
    ));
    spec.install.entries.push(gaia_spec::InstallEntrySpec {
        id: "install-smoke-app".into(),
        artifact: gaia_spec::ArtifactRef::new("smoke-app"),
        dest: "/usr/bin/smoke-app".into(),
        replace: true,
        mode: Some(0o755),
        owner: Some("root".into()),
        group: Some("root".into()),
    });
    spec.stage.files.push(gaia_spec::StageFileSpec {
        id: "motd".into(),
        src: "assets/motd".into(),
        dest: "/etc/motd".into(),
        origin: gaia_spec::StageContentOriginSpec::StaticAsset,
    });
    spec.stage.env_sets.push(gaia_spec::StageEnvSetSpec {
        id: "runtime-env".into(),
        name: "runtime".into(),
        entries: vec![
            ("MODE".into(), "smoke".into()),
            ("ENABLED".into(), "true".into()),
        ],
    });
    spec.stage.services.push(gaia_spec::StageServiceSpec {
        id: "gaia-service".into(),
        name: "gaia-smoke.service".into(),
        unit_path: "assets/gaia-smoke.service".into(),
    });

    let mut image = ImageSpec::new(ImageDefinition::Buildroot(BuildrootImageSpec::default()));
    image.feed.install_entries.push("install-smoke-app".into());
    image.feed.stage_files.push("motd".into());
    image.feed.stage_env_sets.push("runtime-env".into());
    image.feed.stage_services.push("gaia-service".into());

    apply_image_feed_to_rootfs(&spec, &image, &rootfs_dir).expect("feed overlay");

    assert_eq!(
        fs::read_to_string(rootfs_dir.join("usr/bin/smoke-app")).expect("installed artifact"),
        "binary"
    );
    assert_eq!(
        fs::read_to_string(rootfs_dir.join("etc/motd")).expect("staged motd"),
        "hello motd"
    );
    assert_eq!(
        fs::read_to_string(rootfs_dir.join("etc/default/runtime.env")).expect("env set"),
        "MODE=smoke\nENABLED=true\n"
    );
    assert!(
        fs::read_to_string(rootfs_dir.join("etc/systemd/system/gaia-smoke.service"))
            .expect("service file")
            .contains("ExecStart=/usr/bin/smoke-app")
    );
}

#[test]
fn final_tar_image_contains_install_stage_env_and_service_content() {
    let workspace_root = temp_path("gaia-buildroot-final-image-workspace");
    let rootfs_dir = temp_path("gaia-buildroot-final-image-rootfs");
    let output_dir = temp_path("gaia-buildroot-final-image-output");
    let collect_dir = temp_path("gaia-buildroot-final-image-collect");
    let archive_path = temp_path("gaia-buildroot-final-image-primary.tar");
    let artifact_dir = workspace_root.join("out/artifacts");
    let assets_dir = workspace_root.join("assets");
    fs::create_dir_all(&artifact_dir).expect("artifact dir");
    fs::create_dir_all(&assets_dir).expect("assets dir");

    let artifact_path = artifact_dir.join("smoke-app");
    let stage_file_path = assets_dir.join("motd");
    let service_path = assets_dir.join("gaia-smoke.service");
    fs::write(&artifact_path, "binary").expect("artifact file");
    fs::write(&stage_file_path, "hello motd").expect("stage file");
    fs::write(&service_path, "[Service]\nExecStart=/usr/bin/smoke-app\n").expect("service");

    let mut spec = ResolvedBuildSpec::new("buildroot-final-image-test");
    spec.workspace.root_dir = workspace_root.display().to_string();
    spec.artifacts.push(gaia_spec::ArtifactSpec::new(
        "smoke-app",
        gaia_spec::ArtifactDefinition::Rust(gaia_spec::RustArtifactSpec {
            package: "smoke-app".into(),
            target_name: None,
            variant: gaia_spec::ArtifactVariantSpec::File,
        }),
        None,
        gaia_spec::ArtifactOutputSpec {
            path: artifact_path.display().to_string(),
        },
    ));
    spec.install.entries.push(gaia_spec::InstallEntrySpec {
        id: "install-smoke-app".into(),
        artifact: gaia_spec::ArtifactRef::new("smoke-app"),
        dest: "/usr/bin/smoke-app".into(),
        replace: true,
        mode: Some(0o755),
        owner: Some("root".into()),
        group: Some("root".into()),
    });
    spec.stage.files.push(gaia_spec::StageFileSpec {
        id: "motd".into(),
        src: "assets/motd".into(),
        dest: "/etc/motd".into(),
        origin: gaia_spec::StageContentOriginSpec::StaticAsset,
    });
    spec.stage.env_sets.push(gaia_spec::StageEnvSetSpec {
        id: "runtime-env".into(),
        name: "runtime".into(),
        entries: vec![("MODE".into(), "smoke".into())],
    });
    spec.stage.services.push(gaia_spec::StageServiceSpec {
        id: "gaia-service".into(),
        name: "gaia-smoke.service".into(),
        unit_path: "assets/gaia-smoke.service".into(),
    });

    let mut image = ImageSpec::new(ImageDefinition::Buildroot(BuildrootImageSpec {
        expected_images: vec![BuildrootExpectedImageSpec {
            name: "rootfs.tar".into(),
            format: BuildrootExpectedImageFormatSpec::Tar,
            required: true,
        }],
        ..BuildrootImageSpec::default()
    }));
    image.feed.install_entries.push("install-smoke-app".into());
    image.feed.stage_files.push("motd".into());
    image.feed.stage_env_sets.push("runtime-env".into());
    image.feed.stage_services.push("gaia-service".into());

    apply_image_feed_to_rootfs(&spec, &image, &rootfs_dir).expect("feed overlay");
    refresh_expected_tar_images(&image, &rootfs_dir, &output_dir, &test_execution())
        .expect("expected tar image");
    let matched =
        collect_expected_images(&image, &output_dir, &collect_dir).expect("collect expected image");
    let mut reuse_details = Vec::new();
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();
    archive_buildroot_output(BuildrootArchiveRequest {
        image: &image,
        collect_dir: &collect_dir,
        output_dir: &output_dir,
        matched_expected_images: &matched,
        archive_path: &archive_path,
        reuse_details: &mut reuse_details,
        command: test_command_context(&execution, &policy),
    })
    .expect("promote primary image");

    let listing = Command::new("tar")
        .arg("-tf")
        .arg(&archive_path)
        .output()
        .expect("tar listing");
    assert!(listing.status.success(), "archive listing should succeed");
    let listing = String::from_utf8_lossy(&listing.stdout);
    assert!(listing.contains("./usr/bin/smoke-app"));
    assert!(listing.contains("./etc/motd"));
    assert!(listing.contains("./etc/default/runtime.env"));
    assert!(listing.contains("./etc/systemd/system/gaia-smoke.service"));

    let motd = Command::new("tar")
        .arg("-xOf")
        .arg(&archive_path)
        .arg("./etc/motd")
        .output()
        .expect("extract motd");
    assert_eq!(String::from_utf8_lossy(&motd.stdout), "hello motd");

    let env_file = Command::new("tar")
        .arg("-xOf")
        .arg(&archive_path)
        .arg("./etc/default/runtime.env")
        .output()
        .expect("extract env file");
    assert_eq!(String::from_utf8_lossy(&env_file.stdout), "MODE=smoke\n");

    let service = Command::new("tar")
        .arg("-xOf")
        .arg(&archive_path)
        .arg("./etc/systemd/system/gaia-smoke.service")
        .output()
        .expect("extract service");
    assert!(String::from_utf8_lossy(&service.stdout).contains("ExecStart=/usr/bin/smoke-app"));
}

fn fake_elf(machine: u16) -> Vec<u8> {
    let mut bytes = vec![0u8; 64];
    bytes[0..4].copy_from_slice(b"\x7FELF");
    bytes[4] = 2;
    bytes[5] = 1;
    bytes[6] = 1;
    bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&machine.to_le_bytes());
    bytes
}

#[test]
fn apply_image_feed_to_rootfs_rejects_wrong_target_artifact_binary() {
    let workspace_root = temp_path("gaia-buildroot-feed-target-workspace");
    let rootfs_dir = temp_path("gaia-buildroot-feed-target-rootfs");
    let artifact_dir = workspace_root.join("out/artifacts");
    fs::create_dir_all(&artifact_dir).expect("artifact dir");

    let artifact_path = artifact_dir.join("smoke-app");
    fs::write(&artifact_path, fake_elf(0x3E)).expect("x86_64 artifact");

    let mut spec = ResolvedBuildSpec::new("buildroot-feed-target-test");
    spec.workspace.root_dir = workspace_root.display().to_string();
    let mut artifact = gaia_spec::ArtifactSpec::new(
        "smoke-app",
        gaia_spec::ArtifactDefinition::Rust(gaia_spec::RustArtifactSpec {
            package: "smoke-app".into(),
            target_name: None,
            variant: gaia_spec::ArtifactVariantSpec::File,
        }),
        None,
        gaia_spec::ArtifactOutputSpec {
            path: artifact_path.display().to_string(),
        },
    );
    artifact.target = Some("aarch64-unknown-linux-gnu".into());
    spec.artifacts.push(artifact);
    spec.install.entries.push(gaia_spec::InstallEntrySpec {
        id: "install-smoke-app".into(),
        artifact: gaia_spec::ArtifactRef::new("smoke-app"),
        dest: "/usr/bin/smoke-app".into(),
        replace: true,
        mode: Some(0o755),
        owner: Some("root".into()),
        group: Some("root".into()),
    });

    let mut image = ImageSpec::new(ImageDefinition::Buildroot(BuildrootImageSpec::default()));
    image.feed.install_entries.push("install-smoke-app".into());

    let error = apply_image_feed_to_rootfs(&spec, &image, &rootfs_dir)
        .expect_err("wrong-target artifact should be rejected");

    assert_eq!(error.kind, ImageProviderErrorKind::PolicyBlocked);
    assert!(error.message.contains("target mismatch"));
    assert!(error.message.contains("aarch64-unknown-linux-gnu"));
    assert!(error.message.contains("x86_64"));
}
