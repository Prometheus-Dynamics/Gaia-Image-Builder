use super::*;

#[test]
fn command_for_execution_wraps_docker_backend() {
    let workspace_root = temp_path("gaia-buildroot-docker-workspace");
    let buildroot_dir = workspace_root.join("buildroot");
    let output_dir = workspace_root.join("output");
    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::create_dir_all(&output_dir).expect("output dir");
    let execution = ImageExecutionContext {
        workspace_root: workspace_root.clone(),
        docker_image: Some("docker.io/library/alpine:latest".to_string()),
    };
    let mut command = Command::new("make");
    command
        .arg(format!("O={}", output_dir.display()))
        .arg("target-post-image")
        .current_dir(&buildroot_dir);

    let wrapped = command_for_execution(&command, &execution).expect("wrapped command");
    let args = wrapped
        .get_args()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>();

    assert_eq!(wrapped.get_program(), std::ffi::OsStr::new("docker"));
    assert!(args.contains(&"docker.io/library/alpine:latest".to_string()));
    assert!(args.contains(&"make".to_string()));
    assert!(args.contains(&"target-post-image".to_string()));
}

#[test]
fn execute_image_refuses_implicit_buildroot_fallback() {
    let workspace_root = temp_path("gaia-buildroot-no-fallback-workspace");
    let collect_dir = temp_path("gaia-buildroot-no-fallback-collect");
    let spec = ResolvedBuildSpec {
        workspace: gaia_spec::WorkspaceSpec {
            root_dir: workspace_root.display().to_string(),
            build_dir: "build".into(),
            out_dir: "out".into(),
            clean_policy: gaia_spec::CleanPolicy::None,
            named_paths: Vec::new(),
        },
        ..ResolvedBuildSpec::new("no-fallback")
    };
    let image = ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            allow_fallback: false,
            expected_images: vec![BuildrootExpectedImageSpec {
                name: "rootfs.tar".into(),
                format: BuildrootExpectedImageFormatSpec::Tar,
                required: true,
            }],
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec {
            collect_dir: Some(collect_dir.display().to_string()),
            archive_name: Some("primary.tar".into()),
            emit_report: true,
        },
        assembly: None,
    };

    let error = BuildrootImageProvider
        .execute_image(
            &spec,
            &image,
            &ImageOutputContract {
                collect_dir: Some(collect_dir.display().to_string()),
                archive_name: Some("primary.tar".into()),
                emit_report: true,
            },
            &ImageExecutionPolicy::default(),
            None,
            None,
        )
        .expect_err("implicit fallback should be refused");

    assert_eq!(error.kind, ImageProviderErrorKind::OutputMissing);
    assert!(error.message.contains("allow_fallback is false"));
}

#[test]
fn prepare_operation_reuses_existing_expected_buildroot_images() {
    let workspace_root = temp_path("gaia-buildroot-prepare-reuse-workspace");
    let build_dir = workspace_root.join("build");
    let source_dir = build_dir.join("sources").join("buildroot-source");
    let collect_dir = workspace_root.join("out").join("images");
    let output_dir = collect_dir.join("buildroot-output");
    let target_dir = output_dir.join("target");
    let images_dir = output_dir.join("images");

    fs::create_dir_all(&source_dir).expect("source dir");
    fs::create_dir_all(&target_dir).expect("target dir");
    fs::create_dir_all(&images_dir).expect("images dir");
    fs::write(source_dir.join("Makefile"), "all:\n\t@true\n").expect("buildroot makefile");
    fs::write(target_dir.join("marker"), "prepared").expect("prepared marker");
    fs::write(images_dir.join("rootfs.tar"), "image").expect("expected image");

    let mut spec = ResolvedBuildSpec::new("buildroot-prepare-reuse");
    spec.workspace.root_dir = workspace_root.display().to_string();
    spec.workspace.build_dir = "build".into();
    spec.workspace.out_dir = "out".into();

    let image = ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            source: Some(SourceId::new("buildroot-source")),
            defconfig: Some("raspberrypicm5io_defconfig".into()),
            expected_images: vec![BuildrootExpectedImageSpec {
                name: "rootfs.tar".into(),
                format: BuildrootExpectedImageFormatSpec::Tar,
                required: true,
            }],
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec {
            collect_dir: Some(collect_dir.display().to_string()),
            archive_name: None,
            emit_report: false,
        },
        assembly: None,
    };

    let result = BuildrootImageProvider
        .execute_image_operation(gaia_image_providers::ImageOperationExecution {
            spec: &spec,
            image: &image,
            operation: ImageProviderOperation::Prepare,
            output: &ImageOutputContract {
                collect_dir: Some(collect_dir.display().to_string()),
                archive_name: None,
                emit_report: false,
            },
            policy: &ImageExecutionPolicy::default(),
            log_sink: None,
            cancel_check: None,
        })
        .expect("prepare should reuse existing expected image output");

    assert!(result.messages.iter().any(|message| {
        message.contains("reused prepared buildroot output") && message.contains("buildroot-output")
    }));
    assert!(target_dir.join("marker").is_file());
}

#[test]
fn prepare_operation_does_not_reuse_partial_target_tree() {
    let workspace_root = temp_path("gaia-buildroot-prepare-partial-workspace");
    let build_dir = workspace_root.join("build");
    let source_dir = build_dir.join("sources").join("buildroot-source");
    let collect_dir = workspace_root.join("out").join("images");
    let output_dir = collect_dir.join("buildroot-output");
    let target_dir = output_dir.join("target");

    fs::create_dir_all(&source_dir).expect("source dir");
    fs::create_dir_all(&target_dir).expect("target dir");
    fs::write(
        source_dir.join("Makefile"),
        "%_defconfig:\n\t@mkdir -p $(O)/target\n\t@echo defconfig > $(O)/target/marker\nall:\n\t@echo make >> $(O)/target/marker\n",
    )
    .expect("buildroot makefile");
    fs::write(target_dir.join("marker"), "stale").expect("partial marker");

    let mut spec = ResolvedBuildSpec::new("buildroot-prepare-partial");
    spec.workspace.root_dir = workspace_root.display().to_string();
    spec.workspace.build_dir = "build".into();
    spec.workspace.out_dir = "out".into();

    let image = ImageSpec {
        definition: ImageDefinition::Buildroot(BuildrootImageSpec {
            source: Some(SourceId::new("buildroot-source")),
            defconfig: Some("raspberrypicm5io_defconfig".into()),
            ..BuildrootImageSpec::default()
        }),
        feed: gaia_spec::ImageFeedSpec::default(),
        output: ImageOutputSpec {
            collect_dir: Some(collect_dir.display().to_string()),
            archive_name: None,
            emit_report: false,
        },
        assembly: None,
    };

    BuildrootImageProvider
        .execute_image_operation(gaia_image_providers::ImageOperationExecution {
            spec: &spec,
            image: &image,
            operation: ImageProviderOperation::Prepare,
            output: &ImageOutputContract {
                collect_dir: Some(collect_dir.display().to_string()),
                archive_name: None,
                emit_report: false,
            },
            policy: &ImageExecutionPolicy::default(),
            log_sink: None,
            cancel_check: None,
        })
        .expect("prepare should rerun buildroot for partial target tree");

    let marker = fs::read_to_string(target_dir.join("marker")).expect("marker");
    assert_eq!(marker, "defconfig\nmake\n");
}

#[test]
fn build_operation_reruns_when_assembly_provider_input_is_missing() {
    let workspace_root = temp_path("gaia-buildroot-assembly-input-workspace");
    let build_dir = workspace_root.join("build");
    let source_dir = build_dir.join("sources").join("buildroot-source");
    let collect_dir = workspace_root.join("out").join("images");
    let output_dir = collect_dir.join("buildroot-output");
    let images_dir = output_dir.join("images");
    let marker = output_dir.join("buildroot-ran");

    fs::create_dir_all(&source_dir).expect("source dir");
    fs::create_dir_all(&images_dir).expect("images dir");
    fs::write(images_dir.join("sdcard.img"), "stale assembly output").expect("assembly output");
    fs::write(
        source_dir.join("Makefile"),
        "%_defconfig:\n\t@mkdir -p $(O)\n\t@echo defconfig > $(O)/buildroot-ran\nall:\n\t@echo all >> $(O)/buildroot-ran\n",
    )
    .expect("buildroot makefile");

    let mut spec = ResolvedBuildSpec::new("buildroot-assembly-input-rerun");
    spec.workspace.root_dir = workspace_root.display().to_string();
    spec.workspace.build_dir = "build".into();
    spec.workspace.out_dir = "out".into();

    let mut image = ImageSpec::new(ImageDefinition::Buildroot(BuildrootImageSpec {
        source: Some(SourceId::new("buildroot-source")),
        defconfig: Some("raspberrypicm5io_defconfig".into()),
        expected_images: vec![BuildrootExpectedImageSpec {
            name: "sdcard.img".into(),
            format: BuildrootExpectedImageFormatSpec::Raw,
            required: true,
        }],
        ..BuildrootImageSpec::default()
    }));
    image.output = ImageOutputSpec {
        collect_dir: Some(collect_dir.display().to_string()),
        archive_name: None,
        emit_report: false,
    };
    image.assembly = Some(ImageAssemblySpec {
        disks: vec![AssemblyDiskSpec {
            id: "sdcard".into(),
            output: "$provider.images/sdcard.img".into(),
            partition_table: AssemblyPartitionTableSpec::Mbr,
            signature: None,
            signature_text: None,
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

    let error = BuildrootImageProvider
        .execute_image_operation(gaia_image_providers::ImageOperationExecution {
            spec: &spec,
            image: &image,
            operation: ImageProviderOperation::Build,
            output: &ImageOutputContract {
                collect_dir: Some(collect_dir.display().to_string()),
                archive_name: None,
                emit_report: false,
            },
            policy: &ImageExecutionPolicy::default(),
            log_sink: None,
            cancel_check: None,
        })
        .expect_err("missing provider-root assembly input should fail after buildroot reruns");

    assert_eq!(error.kind, ImageProviderErrorKind::OutputMissing);
    assert!(error.message.contains("rootfs.tar"), "{}", error.message);
    assert_eq!(
        fs::read_to_string(marker).expect("buildroot marker"),
        "defconfig\nall\n"
    );
}

#[test]
fn direct_squashfs_refresh_reuses_prepared_target_without_make() {
    let workspace_root = temp_path("gaia-buildroot-direct-squashfs-workspace");
    let buildroot_dir = workspace_root.join("buildroot");
    let output_dir = workspace_root.join("out/buildroot-output");
    let source_target_dir = output_dir.join("target");
    let squashfs_dir = output_dir.join("build/buildroot-fs/squashfs");
    let staged_target_dir = squashfs_dir.join("target");
    let working_target_dir = squashfs_dir.join("target.refresh");
    let fakeroot_script = squashfs_dir.join("fakeroot");
    let host_bin_dir = output_dir.join("host/bin");
    let image_path = output_dir.join("images/rootfs.squashfs");

    fs::create_dir_all(&buildroot_dir).expect("buildroot dir");
    fs::create_dir_all(&source_target_dir).expect("source target dir");
    fs::create_dir_all(&host_bin_dir).expect("host bin dir");
    fs::create_dir_all(image_path.parent().expect("image parent")).expect("images dir");
    fs::create_dir_all(&squashfs_dir).expect("squashfs dir");
    fs::create_dir_all(&staged_target_dir).expect("staged target dir");

    fs::write(buildroot_dir.join("package-marker"), "present").expect("package marker");
    fs::write(source_target_dir.join("etc-motd"), "hello").expect("source target content");
    fs::write(staged_target_dir.join("prepared-marker"), "keep-me").expect("prepared staged file");
    fs::write(
        host_bin_dir.join("fakeroot"),
        "#!/bin/sh\nPATH=\"$(dirname \"$0\"):$PATH\" exec \"$@\"\n",
    )
    .expect("fakeroot wrapper");
    fs::write(host_bin_dir.join("chown"), "#!/bin/sh\nexit 0\n").expect("fake chown");
    fs::write(
        host_bin_dir.join("mksquashfs"),
        "#!/bin/sh\nprintf 'squashfs-image' > \"$2\"\n",
    )
    .expect("mksquashfs wrapper");
    #[cfg(unix)]
    {
        fs::set_permissions(
            host_bin_dir.join("fakeroot"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("fakeroot perms");
        fs::set_permissions(
            host_bin_dir.join("chown"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("chown perms");
        fs::set_permissions(
            host_bin_dir.join("mksquashfs"),
            fs::Permissions::from_mode(0o755),
        )
        .expect("mksquashfs perms");
    }
    fs::write(
            &fakeroot_script,
            format!(
                "#!/bin/sh\nset -e\nchown -h -R 101:102 '{target}/var/empty'\n[ -f \"{pkg}\" ]\n[ -f \"{target}/etc-motd\" ]\n[ -f \"{target}/prepared-marker\" ]\n[ -d \"{target}/var/empty\" ]\n\"{mksquashfs}\" \"{target}\" \"{image}\"\n",
                pkg = buildroot_dir.join("package-marker").display(),
                target = staged_target_dir.display(),
                mksquashfs = host_bin_dir.join("mksquashfs").display(),
                image = image_path.display(),
            ),
        )
        .expect("fakeroot script");
    #[cfg(unix)]
    fs::set_permissions(&fakeroot_script, fs::Permissions::from_mode(0o755))
        .expect("fakeroot script perms");

    let messages = refresh_buildroot_squashfs_images_direct(
        &buildroot_dir,
        &output_dir,
        &test_execution(),
        &ImageExecutionPolicy::default(),
    )
    .expect("direct squashfs refresh")
    .expect("direct squashfs path should be used");

    assert!(messages.iter().any(|message| {
        message.contains("direct squashfs refresh")
            || message.contains("refreshed squashfs image directly")
    }));
    assert_eq!(
        fs::read_to_string(&image_path).expect("squashfs image"),
        "squashfs-image"
    );
    assert_eq!(
        fs::read_to_string(working_target_dir.join("etc-motd")).expect("working copy"),
        "hello"
    );
    assert_eq!(
        fs::read_to_string(working_target_dir.join("prepared-marker"))
            .expect("preserved prepared marker"),
        "keep-me"
    );
    assert!(working_target_dir.join("var/empty").is_dir());
}

#[test]
fn copy_path_preserves_symlinks_in_rootfs_tree() {
    let root = temp_path("gaia-buildroot-copy-symlink");
    let src_dir = root.join("src");
    let dest_dir = root.join("dest");
    fs::create_dir_all(src_dir.join("dev")).expect("src dev dir");
    #[cfg(unix)]
    std::os::unix::fs::symlink("/proc/self/fd", src_dir.join("dev/fd"))
        .expect("create source symlink");

    copy_path(&src_dir, &dest_dir).expect("copy rootfs tree with symlink");

    let copied = dest_dir.join("dev/fd");
    let metadata = fs::symlink_metadata(&copied).expect("copied metadata");
    assert!(metadata.file_type().is_symlink());
    assert_eq!(
        fs::read_link(&copied).expect("copied symlink target"),
        PathBuf::from("/proc/self/fd")
    );
}

#[test]
fn ensure_fakeroot_chown_paths_exist_creates_missing_directories() {
    let root = temp_path("gaia-buildroot-fakeroot-paths");
    let staged_target_dir = root.join("target");
    fs::create_dir_all(&staged_target_dir).expect("staged target dir");
    let fakeroot_script = root.join("fakeroot");
    fs::write(
        &fakeroot_script,
        format!(
            "chown -h -R 101:102 '{}/var/empty'\n",
            staged_target_dir.display()
        ),
    )
    .expect("write fakeroot script");

    ensure_fakeroot_chown_paths_exist(&fakeroot_script, &staged_target_dir)
        .expect("precreate fakeroot-owned directories");

    assert!(staged_target_dir.join("var/empty").is_dir());
}
