use super::*;

#[test]
fn run_buildroot_reports_make_failures() {
    let spec = ResolvedBuildSpec::new("buildroot-test");
    let buildroot_dir = temp_path("gaia-buildroot-test-dir");
    let output_dir = temp_path("gaia-buildroot-test-out");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::write(
            buildroot_dir.join("Makefile"),
            "bad_defconfig:\n\t@echo defconfig failed 1>&2\n\t@false\nall:\n\t@echo build failed 1>&2\n\t@false\n",
        )
        .expect("makefile");
    let image = ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            defconfig: Some("bad_defconfig".into()),
            external_tree: None,
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec {
            collect_dir: None,
            archive_name: None,
            emit_report: true,
        },
    };

    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();
    let error = run_buildroot(BuildrootRunRequest {
        spec: &spec,
        image: &image,
        buildroot_dir: &buildroot_dir,
        output_dir: &output_dir,
        command: test_command_context(&execution, &policy),
    })
    .expect_err("failing buildroot make should error");

    assert_eq!(error.kind, ImageProviderErrorKind::BackendCommand);
    assert!(error.message.contains("buildroot defconfig failed"));
}

#[test]
fn run_command_reports_missing_tool() {
    let error = run_command(
        Command::new("gaia-missing-buildroot-tool"),
        "buildroot archive",
        &test_execution(),
        &ImageExecutionPolicy::default(),
        None,
        None,
    )
    .expect_err("missing tool should fail");

    assert_eq!(error.kind, ImageProviderErrorKind::ToolStart);
    assert!(error.message.contains("failed to start buildroot archive"));
}

#[test]
fn run_command_times_out_long_running_processes() {
    let policy = ImageExecutionPolicy {
        timeout_seconds: 1,
        ..ImageExecutionPolicy::default()
    };
    let mut command = Command::new("sh");
    command.arg("-c").arg("sleep 5");

    let started = Instant::now();
    let error = run_command(
        command,
        "buildroot timeout smoke",
        &test_execution(),
        &policy,
        None,
        None,
    )
    .expect_err("sleep should time out");

    assert_eq!(error.kind, ImageProviderErrorKind::Timeout);
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "timeout should interrupt before the child sleep completes"
    );
}

#[test]
fn make_jobs_are_provider_local_and_not_forced_by_scheduler_jobs() {
    let mut no_local_jobs = Command::new("make");
    append_make_jobs(&mut no_local_jobs, 0);
    assert!(
        !no_local_jobs
            .get_args()
            .any(|arg| arg.to_string_lossy().starts_with("-j"))
    );

    let mut local_jobs = Command::new("make");
    append_make_jobs(&mut local_jobs, 2);
    assert!(local_jobs.get_args().any(|arg| arg == "-j2"));
}

#[test]
fn command_for_execution_mounts_parent_dir_for_output_files() {
    let workspace_root = temp_path("gaia-buildroot-docker-parent");
    let output_path = workspace_root.join("out/rootfs.tar");
    fs::create_dir_all(workspace_root.join("out")).expect("out dir");
    let execution = ImageExecutionContext {
        workspace_root: workspace_root.clone(),
        docker_image: Some("docker.io/library/debian:stable-slim".to_string()),
    };
    let mut command = Command::new("tar");
    command
        .arg("-cf")
        .arg(&output_path)
        .arg("-C")
        .arg(&workspace_root)
        .arg(".");

    let wrapped = command_for_execution(&command, &execution).expect("wrapped command");
    let args = wrapped
        .get_args()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    let mount = format!(
        "{}:{}",
        workspace_root.join("out").display(),
        workspace_root.join("out").display()
    );

    assert!(args.contains(&mount));
    assert!(!args.contains(&format!(
        "{}:{}",
        output_path.display(),
        output_path.display()
    )));
}

#[test]
fn buildroot_local_jobs_are_rendered_as_make_jobs() {
    let mut command = Command::new("make");
    append_make_jobs(&mut command, 2);

    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>();

    assert!(args.contains(&"-j2".to_string()));
}

#[test]
fn buildroot_zero_local_jobs_omits_make_jobs() {
    let mut command = Command::new("make");
    append_make_jobs(&mut command, 0);

    assert_eq!(command.get_args().count(), 0);
}

#[test]
fn refresh_buildroot_images_after_feed_overlay_runs_target_post_image_for_non_tar_outputs() {
    let buildroot_dir = temp_path("gaia-buildroot-post-image-dir");
    let output_dir = temp_path("gaia-buildroot-post-image-out");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::write(
            buildroot_dir.join("Makefile"),
            "target-post-image:\n\t@mkdir -p $(O)/images\n\t@printf 'refreshed' > $(O)/images/rootfs.squashfs\n",
        )
        .expect("makefile");
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

    let messages = refresh_buildroot_images_after_feed_overlay(
        &image,
        &buildroot_dir,
        &output_dir,
        &test_execution(),
        &ImageExecutionPolicy::default(),
        None,
        None,
    )
    .expect("target-post-image refresh should succeed");

    assert!(
        output_dir.join("images/rootfs.squashfs").is_file(),
        "expected target-post-image to refresh non-tar image"
    );
    assert!(messages.is_empty());
}

#[test]
fn refresh_buildroot_post_image_direct_runs_configured_script_with_buildroot_env() {
    let workspace_root = temp_path("gaia-buildroot-direct-post-image-workspace");
    let buildroot_dir = workspace_root.join("buildroot");
    let output_dir = workspace_root.join("out/buildroot-output");
    let script_path = workspace_root.join("post-image.sh");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::create_dir_all(output_dir.join("images")).expect("images dir");
    fs::create_dir_all(output_dir.join("target")).expect("target dir");
    fs::create_dir_all(output_dir.join("build")).expect("build dir");
    fs::create_dir_all(output_dir.join("host")).expect("host dir");
    fs::write(
        output_dir.join(".config"),
        format!(
            "BR2_ROOTFS_POST_IMAGE_SCRIPT=\"{}\"\n",
            script_path.display()
        ),
    )
    .expect("buildroot config");
    fs::write(
        &script_path,
        "#!/bin/sh\nset -e\n[ -d \"$BINARIES_DIR\" ]\n[ -d \"$TARGET_DIR\" ]\nprintf raw > \"$BINARIES_DIR/sdcard.img\"\n",
    )
    .expect("post image script");
    #[cfg(unix)]
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755))
        .expect("post image script perms");

    let messages = refresh_buildroot_post_image_direct(
        &buildroot_dir,
        &output_dir,
        &test_execution(),
        &ImageExecutionPolicy::default(),
        None,
        None,
    )
    .expect("direct post-image should not error")
    .expect("direct post-image should run");

    assert_eq!(
        fs::read_to_string(output_dir.join("images/sdcard.img")).expect("raw image"),
        "raw"
    );
    assert!(
        messages
            .iter()
            .any(|message| { message.contains("refreshed buildroot post-image outputs directly") })
    );
}

#[test]
fn materialize_defconfig_support_files_copies_sibling_assets_into_output_dir() {
    let assets_dir = temp_path("gaia-buildroot-defconfig-assets");
    let output_dir = temp_path("gaia-buildroot-defconfig-out");
    fs::create_dir_all(&assets_dir).expect("assets dir");
    let defconfig_path = assets_dir.join("custom.defconfig");
    fs::write(&defconfig_path, "BR2_x86_64=y\n").expect("defconfig");
    fs::write(assets_dir.join("genimage.cfg"), "image test {}\n").expect("genimage");
    fs::create_dir_all(assets_dir.join("overlays")).expect("overlay dir");
    fs::write(assets_dir.join("overlays/config.txt"), "dtoverlay=test\n").expect("overlay");

    materialize_defconfig_support_files(&defconfig_path, &output_dir)
        .expect("support files should copy");

    assert!(!output_dir.join("custom.defconfig").exists());
    assert_eq!(
        fs::read_to_string(output_dir.join("genimage.cfg")).expect("copied genimage"),
        "image test {}\n"
    );
    assert_eq!(
        fs::read_to_string(output_dir.join("overlays/config.txt")).expect("copied overlay"),
        "dtoverlay=test\n"
    );
}

#[test]
fn apply_buildroot_config_fragments_merges_fragments_and_runs_olddefconfig() {
    let workspace_root = temp_path("gaia-buildroot-fragment-workspace");
    let buildroot_dir = temp_path("gaia-buildroot-fragment-buildroot");
    let output_dir = temp_path("gaia-buildroot-fragment-output");
    fs::create_dir_all(workspace_root.join("assets")).expect("assets dir");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::create_dir_all(&output_dir).expect("output dir");
    fs::write(
        output_dir.join(".config"),
        "BR2_TARGET_ROOTFS_TAR=y\nBR2_PACKAGE_BUSYBOX=n\n",
    )
    .expect("base config");
    fs::write(
        workspace_root.join("assets/fragment-a.cfg"),
        "BR2_PACKAGE_BUSYBOX=y\n",
    )
    .expect("fragment a");
    fs::write(
        workspace_root.join("assets/fragment-b.cfg"),
        "BR2_PACKAGE_DROPBEAR=y\n",
    )
    .expect("fragment b");
    fs::write(
        buildroot_dir.join("Makefile"),
        "olddefconfig:\n\t@printf 'ran\\n' > $(O)/olddefconfig.marker\n",
    )
    .expect("makefile");

    let mut spec = ResolvedBuildSpec::new("buildroot-fragment-merge");
    spec.workspace.root_dir = workspace_root.display().to_string();

    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();
    let messages = apply_buildroot_config_fragments(
        &spec,
        &buildroot_dir,
        &output_dir,
        &[
            "assets/fragment-a.cfg".to_string(),
            "assets/fragment-b.cfg".to_string(),
        ],
        None,
        ImageCommandContext {
            execution: &execution,
            policy: &policy,
            log_sink: None,
            cancel_check: None,
        },
    )
    .expect("fragment merge should succeed");

    let merged = fs::read_to_string(output_dir.join(".config")).expect("merged config");
    assert!(messages.is_empty());
    assert!(merged.contains("BR2_TARGET_ROOTFS_TAR=y"));
    assert!(merged.contains("BR2_PACKAGE_BUSYBOX=y"));
    assert!(merged.contains("BR2_PACKAGE_DROPBEAR=y"));
    assert!(
        output_dir.join("olddefconfig.marker").is_file(),
        "expected olddefconfig to run after merging fragments"
    );
}

#[test]
fn apply_buildroot_config_overrides_merges_overrides_and_runs_olddefconfig() {
    let buildroot_dir = temp_path("gaia-buildroot-override-buildroot");
    let output_dir = temp_path("gaia-buildroot-override-output");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::create_dir_all(&output_dir).expect("output dir");
    fs::write(
        output_dir.join(".config"),
        "BR2_TARGET_ROOTFS_TAR=y\nBR2_PACKAGE_BUSYBOX=n\n",
    )
    .expect("base config");
    fs::write(
        buildroot_dir.join("Makefile"),
        "olddefconfig:\n\t@printf 'ran\\n' > $(O)/olddefconfig.marker\n",
    )
    .expect("makefile");

    let spec = ResolvedBuildSpec::new("override-test");
    let overrides = [
        ("BR2_PACKAGE_BUSYBOX".to_string(), "y".to_string()),
        (
            "BR2_TARGET_GENERIC_HOSTNAME".to_string(),
            "\"gaia\"".to_string(),
        ),
    ];
    let execution = test_execution();
    let policy = ImageExecutionPolicy::default();
    let messages = apply_buildroot_config_overrides(BuildrootConfigOverrideRequest {
        spec: &spec,
        output_dir: &output_dir,
        overrides: &overrides,
        external_tree: None,
        buildroot_dir: &buildroot_dir,
        command: test_command_context(&execution, &policy),
    })
    .expect("override merge should succeed");

    let merged = fs::read_to_string(output_dir.join(".config")).expect("merged config");
    assert!(messages.is_empty());
    assert!(merged.contains("BR2_PACKAGE_BUSYBOX=y"));
    assert!(!merged.contains("BR2_PACKAGE_BUSYBOX=n"));
    assert!(merged.contains("BR2_TARGET_GENERIC_HOSTNAME=\"gaia\""));
    assert!(
        output_dir.join("olddefconfig.marker").is_file(),
        "expected olddefconfig to run after applying overrides"
    );
}

#[test]
fn normalize_global_patch_dir_resolves_workspace_relative_entries() {
    let workspace_root = temp_path("gaia-buildroot-global-patch-workspace");
    let patch_dir = workspace_root.join("gaia/assets/buildroot/patches");
    fs::create_dir_all(&patch_dir).expect("patch dir");
    let mut spec = ResolvedBuildSpec::new("global-patch-dir");
    spec.workspace.root_dir = workspace_root.display().to_string();

    let normalized = normalize_global_patch_dir_value(
        &spec,
        "\"board/raspberrypi/patches gaia/assets/buildroot/patches\"",
    );

    assert_eq!(
        normalized,
        format!("\"board/raspberrypi/patches {}\"", patch_dir.display())
    );
}

#[test]
fn refresh_buildroot_images_after_feed_overlay_skips_tar_only_outputs() {
    let buildroot_dir = temp_path("gaia-buildroot-post-image-skip-dir");
    let output_dir = temp_path("gaia-buildroot-post-image-skip-out");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::write(
        buildroot_dir.join("Makefile"),
        "target-post-image:\n\t@false\n",
    )
    .expect("makefile");
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

    let messages = refresh_buildroot_images_after_feed_overlay(
        &image,
        &buildroot_dir,
        &output_dir,
        &test_execution(),
        &ImageExecutionPolicy::default(),
        None,
        None,
    )
    .expect("tar-only outputs should skip refresh");

    assert!(messages.is_empty());
    assert!(!output_dir.exists());
}

#[test]
fn refresh_buildroot_images_after_feed_overlay_reports_target_post_image_failure() {
    let buildroot_dir = temp_path("gaia-buildroot-post-image-fail-dir");
    let output_dir = temp_path("gaia-buildroot-post-image-fail-out");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::write(
        buildroot_dir.join("Makefile"),
        "target-post-image:\n\t@echo broken post-image 1>&2\n\t@false\n",
    )
    .expect("makefile");
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

    let error = refresh_buildroot_images_after_feed_overlay(
        &image,
        &buildroot_dir,
        &output_dir,
        &test_execution(),
        &ImageExecutionPolicy::default(),
        None,
        None,
    )
    .expect_err("broken target-post-image should fail");

    assert_eq!(error.kind, ImageProviderErrorKind::BackendCommand);
    assert!(error.message.contains("broken post-image"));
}
