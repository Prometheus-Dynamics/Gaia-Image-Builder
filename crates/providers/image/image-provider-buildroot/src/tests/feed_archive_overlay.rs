use super::*;

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
        mode: Some(0o755),
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
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(rootfs_dir.join("etc/motd"))
            .expect("staged motd metadata")
            .permissions()
            .mode()
            & 0o777,
        0o755
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
fn prunes_removed_stage_file_from_previous_runtime_state() {
    let workspace_root = temp_path("gaia-buildroot-feed-prune-workspace");
    let rootfs_dir = temp_path("gaia-buildroot-feed-prune-rootfs");
    let output_dir = temp_path("gaia-buildroot-feed-prune-output");
    let runtime_dir = workspace_root.join("out/.gaia/runtime");
    fs::create_dir_all(rootfs_dir.join("usr/local/bin")).expect("rootfs bin");
    fs::create_dir_all(&runtime_dir).expect("runtime dir");
    fs::write(rootfs_dir.join("usr/local/bin/old.sh"), "stale").expect("stale file");
    fs::write(
        runtime_dir.join("stage-file-old-script.state"),
        "kind=stage-file\nitem_id=old-script\nsrc=assets/old.sh\ndest=/usr/local/bin/old.sh\norigin=static-asset\n",
    )
    .expect("runtime state");

    let mut spec = ResolvedBuildSpec::new("buildroot-feed-prune-test");
    spec.workspace.root_dir = workspace_root.display().to_string();
    spec.workspace.out_dir = workspace_root.join("out").display().to_string();
    let image = ImageSpec::new(ImageDefinition::Buildroot(BuildrootImageSpec::default()));

    prune_stale_image_feed_outputs(&spec, &image, &rootfs_dir, &output_dir)
        .expect("prune stale feed");

    assert!(!rootfs_dir.join("usr/local/bin/old.sh").exists());
}

#[test]
fn stale_image_feed_outputs_are_pruned_before_overlay() {
    let workspace_root = temp_path("gaia-buildroot-feed-prune-workspace");
    let rootfs_dir = temp_path("gaia-buildroot-feed-prune-rootfs");
    let output_dir = temp_path("gaia-buildroot-feed-prune-output");
    let assets_dir = workspace_root.join("assets");
    fs::create_dir_all(&assets_dir).expect("assets dir");
    fs::create_dir_all(rootfs_dir.join("usr/local/bin")).expect("rootfs bin");
    fs::create_dir_all(&output_dir).expect("output dir");
    fs::write(rootfs_dir.join("usr/local/bin/old.sh"), "stale").expect("stale rootfs file");
    fs::write(assets_dir.join("new"), "fresh").expect("fresh asset");
    fs::write(
        image_feed_managed_paths_path(&output_dir),
        "gaia-image-feed-managed-paths-v1\n/usr/local/bin/old.sh\n/etc/new\n",
    )
    .expect("managed paths");

    let mut spec = ResolvedBuildSpec::new("buildroot-feed-prune-test");
    spec.workspace.root_dir = workspace_root.display().to_string();
    spec.stage.files.push(gaia_spec::StageFileSpec {
        id: "new".into(),
        src: "assets/new".into(),
        dest: "/etc/new".into(),
        mode: None,
        origin: gaia_spec::StageContentOriginSpec::StaticAsset,
    });
    let mut image = ImageSpec::new(ImageDefinition::Buildroot(BuildrootImageSpec::default()));
    image.feed.stage_files.push("new".into());

    prune_stale_image_feed_outputs(&spec, &image, &rootfs_dir, &output_dir).expect("prune");
    apply_image_feed_to_rootfs(&spec, &image, &rootfs_dir).expect("feed overlay");
    write_image_feed_managed_paths(&output_dir, &spec, &image).expect("managed paths");

    assert!(!rootfs_dir.join("usr/local/bin/old.sh").exists());
    assert_eq!(
        fs::read_to_string(rootfs_dir.join("etc/new")).expect("new file"),
        "fresh"
    );
    assert_eq!(
        fs::read_to_string(image_feed_managed_paths_path(&output_dir)).expect("managed paths"),
        "gaia-image-feed-managed-paths-v1\n/etc/new\n"
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
        mode: Some(0o755),
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
