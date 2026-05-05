use super::*;

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
        assembly: None,
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
        assembly: None,
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
fn archive_buildroot_output_compresses_raw_board_image_for_img_xz_output() {
    let output_dir = temp_path("gaia-buildroot-raw-board-xz-out");
    let collect_dir = temp_path("gaia-buildroot-raw-board-xz-collect");
    let archive_path = collect_dir.join("helios.img.xz");
    fs::create_dir_all(output_dir.join("images")).expect("images dir");
    fs::write(output_dir.join("images/rootfs.squashfs"), "rootfs").expect("rootfs image");
    fs::write(output_dir.join("images/sdcard.img"), "raw image").expect("raw image");
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
        assembly: None,
    };

    let matched = collect_expected_images(&image, &output_dir, &collect_dir)
        .expect("raw image should be collected");
    assert_eq!(
        matched,
        vec!["rootfs.squashfs".to_string(), "sdcard.img".to_string()]
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
    .expect("raw image should be compressed");

    let output = Command::new("xz")
        .arg("-dc")
        .arg(&archive_path)
        .output()
        .expect("xz decompress");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "raw image");
}

#[test]
fn compress_primary_image_timeout_preserves_existing_archive() {
    let collect_dir = temp_path("gaia-buildroot-raw-board-timeout-collect");
    let script_dir = temp_path("gaia-buildroot-fake-xz-timeout-bin");
    let archive_path = collect_dir.join("helios.img.xz");
    let source_path = collect_dir.join("sdcard.img");
    fs::create_dir_all(&collect_dir).expect("collect dir");
    fs::write(&source_path, "raw image").expect("raw image");
    fs::write(&archive_path, "previous archive").expect("previous archive");
    let fake_xz = fake_xz_script(&script_dir, "xz-timeout", "#!/bin/sh\nsleep 30\n");
    let execution = test_execution();
    let policy = ImageExecutionPolicy {
        timeout_seconds: 1,
        ..ImageExecutionPolicy::default()
    };

    let started = Instant::now();
    let error = compress_primary_image_with_program(
        &fake_xz,
        &source_path,
        &archive_path,
        &execution,
        &policy,
        None,
        None,
    )
    .expect_err("compression should time out");

    assert_eq!(error.kind, ImageProviderErrorKind::Timeout);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "timeout should be bounded"
    );
    assert_eq!(
        fs::read_to_string(&archive_path).expect("archive contents"),
        "previous archive"
    );
    assert!(
        !temporary_archive_output_path(&archive_path).exists(),
        "temporary archive should be removed"
    );
}

#[test]
fn compress_primary_image_failure_preserves_existing_archive() {
    let collect_dir = temp_path("gaia-buildroot-raw-board-failed-collect");
    let script_dir = temp_path("gaia-buildroot-fake-xz-failed-bin");
    let archive_path = collect_dir.join("helios.img.xz");
    let source_path = collect_dir.join("sdcard.img");
    fs::create_dir_all(&collect_dir).expect("collect dir");
    fs::write(&source_path, "raw image").expect("raw image");
    fs::write(&archive_path, "previous archive").expect("previous archive");
    let fake_xz = fake_xz_script(
        &script_dir,
        "xz-failed",
        "#!/bin/sh\nprintf partial\nprintf 'bad xz\\n' >&2\nexit 7\n",
    );
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();

    let error = compress_primary_image_with_program(
        &fake_xz,
        &source_path,
        &archive_path,
        &execution,
        &policy,
        None,
        None,
    )
    .expect_err("compression should fail");

    assert_eq!(error.kind, ImageProviderErrorKind::BackendCommand);
    assert!(error.message.contains("bad xz"));
    assert_eq!(
        fs::read_to_string(&archive_path).expect("archive contents"),
        "previous archive"
    );
    assert!(
        !temporary_archive_output_path(&archive_path).exists(),
        "temporary archive should be removed"
    );
}

#[test]
fn compress_primary_image_streams_logs_and_uses_policy_threads() {
    let collect_dir = temp_path("gaia-buildroot-raw-board-log-collect");
    let script_dir = temp_path("gaia-buildroot-fake-xz-log-bin");
    let archive_path = collect_dir.join("helios.img.xz");
    let source_path = collect_dir.join("sdcard.img");
    let args_path = script_dir.join("args.txt");
    fs::create_dir_all(&collect_dir).expect("collect dir");
    fs::write(&source_path, "raw image").expect("raw image");
    let fake_xz = fake_xz_script(
        &script_dir,
        "xz-log",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\nprintf compressed\nprintf 'compressing\\n' >&2\n",
            args_path.display()
        ),
    );
    let execution = test_execution();
    let policy = ImageExecutionPolicy {
        local_jobs: 2,
        ..ImageExecutionPolicy::default()
    };
    let logs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = std::sync::Arc::clone(&logs);
    let log_sink: ProcessLogSink = std::sync::Arc::new(move |line| {
        captured.lock().expect("logs").push(line.line);
    });

    compress_primary_image_with_program(
        &fake_xz,
        &source_path,
        &archive_path,
        &execution,
        &policy,
        Some(log_sink),
        None,
    )
    .expect("compression should succeed");

    assert_eq!(
        fs::read_to_string(&archive_path).expect("archive contents"),
        "compressed"
    );
    assert!(
        fs::read_to_string(args_path)
            .expect("fake xz args")
            .contains("-T2")
    );
    assert!(
        logs.lock()
            .expect("logs")
            .iter()
            .any(|line| line.contains("compressing"))
    );
}

fn fake_xz_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    fs::create_dir_all(dir).expect("script dir");
    let path = dir.join(name);
    fs::write(&path, body).expect("script body");
    let mut permissions = fs::metadata(&path).expect("script metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("script permissions");
    path
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
        assembly: None,
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
        assembly: None,
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
        assembly: None,
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
