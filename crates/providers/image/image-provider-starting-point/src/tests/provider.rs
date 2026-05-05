use super::*;

#[test]
fn starting_point_tar_rootfs_accepts_image_feed_overlay() {
    let root = unique_dir("gaia-starting-point-overlay");
    let seed_dir = root.join("seed");
    let seed_rootfs = seed_dir.join("rootfs");
    fs::create_dir_all(seed_rootfs.join("etc")).expect("seed etc");
    fs::write(seed_rootfs.join("etc/os-release"), "NAME=seed\n").expect("os-release");
    let seed_tar = root.join("seed-rootfs.tar");
    let status = Command::new("tar")
        .arg("-cf")
        .arg(&seed_tar)
        .arg("-C")
        .arg(&seed_dir)
        .arg("rootfs")
        .status()
        .expect("create tar");
    assert!(status.success());
    let materialized_source = root.join("build/sources/base-rootfs");
    fs::create_dir_all(&materialized_source).expect("materialized source dir");
    fs::copy(&seed_tar, materialized_source.join("seed-rootfs.tar")).expect("copy seed tar");

    let artifact_out = root.join("artifact-out/smoke-app");
    fs::create_dir_all(artifact_out.parent().expect("artifact parent")).expect("artifact dir");
    fs::write(&artifact_out, "binary").expect("artifact output");
    let motd = root.join("motd");
    fs::write(&motd, "hello motd\n").expect("motd");
    let service = root.join("gaia.service");
    fs::write(&service, "[Service]\nExecStart=/usr/bin/smoke-app\n").expect("service");

    let mut spec = gaia_spec::ResolvedBuildSpec::new("starting-point-overlay");
    spec.workspace.root_dir = root.display().to_string();
    spec.workspace.build_dir = "build".into();
    spec.image = ImageSpec {
        definition: ImageDefinition::StartingPoint(StartingPointImageSpec {
            source: Some("base-rootfs".into()),
            source_path: Some("seed-rootfs.tar".into()),
            rootfs_path: String::new(),
            image_partition: None,
            image_read_only: true,
            packages: gaia_spec::StartingPointPackagesSpec::default(),
            rootfs_validation_mode: gaia_spec::StartingPointRootfsValidationModeSpec::RequireFile,
            output_mode: gaia_spec::StartingPointOutputModeSpec::CopyAndArchive,
        }),
        feed: gaia_spec::ImageFeedSpec {
            install_entries: vec!["install-smoke-app".into()],
            stage_files: vec!["motd".into()],
            stage_env_sets: vec!["runtime-env".into()],
            stage_services: vec!["runtime-service".into()],
        },
        output: ImageOutputSpec {
            collect_dir: Some(root.join("out/images").display().to_string()),
            archive_name: Some("overlay.tar".into()),
            emit_report: true,
        },
        assembly: None,
    };
    spec.sources.push(SourceSpec::new(
        "base-rootfs",
        SourceDefinition::Path(gaia_spec::PathSourceSpec {
            path: ".".into(),
            identity_ignore: Vec::new(),
            refresh_policy: gaia_spec::SourceRefreshPolicySpec::Never,
            pin_policy: gaia_spec::SourcePinPolicySpec::Locked,
        }),
    ));
    spec.artifacts.push(ArtifactSpec::new(
        "smoke-app",
        ArtifactDefinition::Rust(RustArtifactSpec {
            package: "smoke-app".into(),
            target_name: None,
            variant: ArtifactVariantSpec::File,
        }),
        Some(SourceRef::new("base-rootfs")),
        ArtifactOutputSpec {
            path: artifact_out.display().to_string(),
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
    spec.stage.files.push(StageFileSpec {
        id: "motd".into(),
        src: motd.display().to_string(),
        dest: "/etc/motd".into(),
        origin: StageContentOriginSpec::StaticAsset,
    });
    spec.stage.env_sets.push(StageEnvSetSpec {
        id: "runtime-env".into(),
        name: "runtime".into(),
        entries: vec![("MODE".into(), "smoke".into())],
    });
    spec.stage.services.push(StageServiceSpec {
        id: "runtime-service".into(),
        name: "gaia.service".into(),
        unit_path: service.display().to_string(),
    });

    let output = ImageOutputContract {
        collect_dir: spec.image.output.collect_dir.clone(),
        archive_name: spec.image.output.archive_name.clone(),
        emit_report: true,
    };
    let provider = StartingPointImageProvider;
    let result = provider
        .execute_image(
            &spec,
            &spec.image,
            &output,
            &ImageExecutionPolicy::default(),
            None,
            None,
        )
        .expect("execute starting-point image");

    let collect_dir = result.collect_dir.expect("collect dir");
    assert!(collect_dir.join("rootfs/etc/os-release").is_file());
    assert_eq!(
        fs::read_to_string(collect_dir.join("rootfs/etc/motd")).expect("motd"),
        "hello motd\n"
    );
    assert_eq!(
        fs::read_to_string(collect_dir.join("rootfs/etc/default/runtime.env"))
            .expect("runtime env"),
        "MODE=smoke\n"
    );
    assert!(
        collect_dir
            .join("rootfs/etc/systemd/system/gaia.service")
            .is_file()
    );
    assert!(collect_dir.join("rootfs/usr/bin/smoke-app").is_file());
    assert!(result.archive_path.expect("archive path").is_file());
}

#[test]
fn starting_point_rejects_overlay_on_opaque_file_rootfs() {
    let root = unique_dir("gaia-starting-point-opaque");
    let opaque = root.join("base.bin");
    fs::write(&opaque, "opaque").expect("opaque rootfs");

    let mut spec = gaia_spec::ResolvedBuildSpec::new("starting-point-opaque");
    spec.workspace.root_dir = root.display().to_string();
    spec.workspace.build_dir = "build".into();
    spec.image = ImageSpec {
        definition: ImageDefinition::StartingPoint(StartingPointImageSpec {
            source: None,
            source_path: None,
            rootfs_path: opaque.display().to_string(),
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
            archive_name: Some("opaque.tar".into()),
            emit_report: true,
        },
        assembly: None,
    };

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
        .expect_err("opaque rootfs should reject overlay");

    assert_eq!(error.kind, ImageProviderErrorKind::PolicyBlocked);
    assert!(error.message.contains("opaque file"));
}
