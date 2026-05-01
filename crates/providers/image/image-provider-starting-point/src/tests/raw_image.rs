use super::*;

#[test]
fn choose_image_partition_prefers_largest_non_vfat_partition() {
    let partitions = vec![
        ImagePartitionInfo {
            path: "/dev/loop0p1".into(),
            fstype: "vfat".into(),
            size_bytes: 128 * 1024 * 1024,
        },
        ImagePartitionInfo {
            path: "/dev/loop0p2".into(),
            fstype: "ext4".into(),
            size_bytes: 64 * 1024 * 1024,
        },
        ImagePartitionInfo {
            path: "/dev/loop0p3".into(),
            fstype: "ext4".into(),
            size_bytes: 96 * 1024 * 1024,
        },
    ];

    let chosen = choose_image_partition(&partitions, None).expect("choose partition");
    assert_eq!(chosen.path, "/dev/loop0p3");
}

#[test]
fn starting_point_raw_image_rejects_read_only_overlay_before_root_check() {
    let root = unique_dir("gaia-starting-point-raw-readonly");
    let image_path = root.join("base.img");
    fs::write(&image_path, "img").expect("image file");

    let mut spec = gaia_spec::ResolvedBuildSpec::new("starting-point-raw");
    spec.workspace.root_dir = root.display().to_string();
    spec.workspace.build_dir = "build".into();
    spec.image = ImageSpec {
        definition: ImageDefinition::StartingPoint(StartingPointImageSpec {
            source: None,
            source_path: None,
            rootfs_path: image_path.display().to_string(),
            image_partition: None,
            image_read_only: true,
            packages: gaia_spec::StartingPointPackagesSpec::default(),
            rootfs_validation_mode: gaia_spec::StartingPointRootfsValidationModeSpec::RequireFile,
            output_mode: gaia_spec::StartingPointOutputModeSpec::ArchiveOnly,
        }),
        feed: gaia_spec::ImageFeedSpec {
            install_entries: vec!["install-smoke-app".into()],
            stage_files: Vec::new(),
            stage_env_sets: Vec::new(),
            stage_services: Vec::new(),
        },
        output: ImageOutputSpec {
            collect_dir: Some(root.join("out/images").display().to_string()),
            archive_name: Some("base-mutated.img".into()),
            emit_report: true,
        },
    };
    spec.artifacts.push(ArtifactSpec::new(
        "smoke-app",
        ArtifactDefinition::Rust(RustArtifactSpec {
            package: "smoke-app".into(),
            target_name: None,
            variant: ArtifactVariantSpec::File,
        }),
        None,
        ArtifactOutputSpec {
            path: root.join("artifact-out/smoke-app").display().to_string(),
        },
    ));
    spec.install.entries.push(InstallEntrySpec {
        id: "install-smoke-app".into(),
        artifact: ArtifactRef::new("smoke-app"),
        dest: "/usr/bin/smoke-app".into(),
        replace: true,
        mode: Some(0o755),
        owner: None,
        group: None,
    });

    let output = ImageOutputContract {
        collect_dir: spec.image.output.collect_dir.clone(),
        archive_name: spec.image.output.archive_name.clone(),
        emit_report: true,
    };
    let provider = StartingPointImageProvider;
    let error = provider
        .execute_image(
            &spec,
            &spec.image,
            &output,
            &ImageExecutionPolicy::default(),
            None,
            None,
        )
        .expect_err("read-only raw image should reject overlay");

    assert_eq!(error.kind, ImageProviderErrorKind::PolicyBlocked);
    assert!(error.message.contains("image_read_only=false"));
}

#[test]
fn cleanup_failure_is_reported_when_primary_work_succeeds() {
    let result = combine_primary_and_cleanup(
        Ok::<_, ImageProviderError>("done"),
        Err(ImageProviderError::runtime_state("cleanup failed")),
    )
    .expect_err("cleanup failure should surface");

    assert_eq!(result.kind, ImageProviderErrorKind::RuntimeState);
    assert!(result.message.contains("cleanup failed"));
}

#[test]
fn cleanup_failure_is_attached_when_primary_work_fails() {
    let result = combine_primary_and_cleanup::<()>(
        Err(ImageProviderError::backend_command("primary failed")),
        Err(ImageProviderError::runtime_state("cleanup failed")),
    )
    .expect_err("primary failure should remain an error");

    assert_eq!(result.kind, ImageProviderErrorKind::BackendCommand);
    assert!(result.message.contains("primary failed"));
    assert!(result.message.contains("additionally cleanup failed"));
    assert!(result.message.contains("cleanup failed"));
}
