pub mod support;

use gaia_exec::{ExecutionCleanupStatus, ExecutionErrorKind, ExecutionProviders, execute_plan};
use gaia_plan::{OperationId, OperationKind, PlannedOperation};
use gaia_spec::{
    AssemblyDiskPartitionSpec, AssemblyDiskSpec, AssemblyFileSpec, AssemblyFilesystemKindSpec,
    AssemblyFilesystemSpec, AssemblyPartitionTableSpec, AssemblyTransformKindSpec,
    AssemblyTransformSpec, AssemblyTreeSpec, ImageAssemblySpec,
};
use std::fs;
use std::path::Path;
use support::{provider_catalogs, test_spec};

#[test]
fn executes_image_assembly_file_staging() {
    let mut spec = test_spec();
    let build_dir = Path::new(&spec.workspace.build_dir);
    let source_dir = build_dir.join("assembly-sources");
    let glob_dir = source_dir.join("firmware");
    fs::create_dir_all(&glob_dir).expect("glob source dir");
    fs::write(source_dir.join("config.txt"), "config").expect("config source");
    fs::write(glob_dir.join("b.dtb"), "b").expect("b dtb");
    fs::write(glob_dir.join("a.dtb"), "a").expect("a dtb");
    #[cfg(unix)]
    std::os::unix::fs::symlink("config.txt", source_dir.join("config-link"))
        .expect("config symlink");

    let tree_dir = build_dir.join("assembly/boot");
    fs::create_dir_all(&tree_dir).expect("preexisting tree");
    fs::write(tree_dir.join("stale"), "stale").expect("stale file");

    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some(build_dir.join("assembly").display().to_string().into()),
        trees: vec![AssemblyTreeSpec {
            id: "boot".into(),
            path: tree_dir.display().to_string().into(),
        }],
        files: vec![
            AssemblyFileSpec {
                tree: "boot".into(),
                src: Some(source_dir.join("config.txt").display().to_string().into()),
                src_glob: None,
                dest: "config.txt".into(),
                mode: Some("0644".into()),
                optional: false,
                preserve_symlink: false,
            },
            AssemblyFileSpec {
                tree: "boot".into(),
                src: None,
                src_glob: Some(glob_dir.join("*.dtb").display().to_string().into()),
                dest: ".".into(),
                mode: None,
                optional: false,
                preserve_symlink: false,
            },
            AssemblyFileSpec {
                tree: "boot".into(),
                src: Some(source_dir.join("missing.txt").display().to_string().into()),
                src_glob: None,
                dest: "missing.txt".into(),
                mode: None,
                optional: true,
                preserve_symlink: false,
            },
            #[cfg(unix)]
            AssemblyFileSpec {
                tree: "boot".into(),
                src: Some(source_dir.join("config-link").display().to_string().into()),
                src_glob: None,
                dest: "config-link".into(),
                mode: None,
                optional: false,
                preserve_symlink: true,
            },
        ],
        ..ImageAssemblySpec::default()
    });

    let plan = gaia_plan::ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![PlannedOperation::new(
            OperationId::image_assembly(),
            OperationKind::AssembleImage,
        )],
    };
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );

    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    assert_eq!(
        fs::read_to_string(tree_dir.join("config.txt")).unwrap(),
        "config"
    );
    assert_eq!(fs::read_to_string(tree_dir.join("a.dtb")).unwrap(), "a");
    assert_eq!(fs::read_to_string(tree_dir.join("b.dtb")).unwrap(), "b");
    assert!(!tree_dir.join("missing.txt").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(
            fs::read_link(tree_dir.join("config-link")).expect("staged symlink"),
            Path::new("config.txt")
        );
        assert_eq!(
            fs::metadata(tree_dir.join("config.txt"))
                .expect("config metadata")
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
    }
    assert!(!tree_dir.join("stale").exists());
    let state = fs::read_to_string(
        Path::new(&spec.workspace.out_dir).join(".gaia/runtime/image-assembly.state"),
    )
    .expect("assembly state");
    #[cfg(unix)]
    assert!(state.contains("staged_file_count=4"));
    #[cfg(not(unix))]
    assert!(state.contains("staged_file_count=3"));
    assert!(state.contains("skipped_file_count=1"));
    assert!(state.contains("file.1.sha256="));
}

#[test]
fn image_assembly_missing_required_source_fails() {
    let mut spec = test_spec();
    let build_dir = Path::new(&spec.workspace.build_dir);
    let tree_dir = build_dir.join("assembly/boot");
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some(build_dir.join("assembly").display().to_string().into()),
        trees: vec![AssemblyTreeSpec {
            id: "boot".into(),
            path: "$assembly.work/boot".into(),
        }],
        files: vec![AssemblyFileSpec {
            tree: "boot".into(),
            src: Some(
                build_dir
                    .join("timed out after 1s missing.txt")
                    .display()
                    .to_string()
                    .into(),
            ),
            src_glob: None,
            dest: "missing.txt".into(),
            mode: None,
            optional: false,
            preserve_symlink: false,
        }],
        ..ImageAssemblySpec::default()
    });
    let plan = gaia_plan::ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![PlannedOperation::new(
            OperationId::image_assembly(),
            OperationKind::AssembleImage,
        )],
    };
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );

    assert_eq!(outcome.errors.len(), 1);
    assert_eq!(outcome.errors[0].code, "assembly_execution_failed");
    assert_eq!(outcome.errors[0].kind, ExecutionErrorKind::RuntimeState);
    assert!(outcome.errors[0].message.contains("does not exist"));
    assert_eq!(
        outcome.errors[0].cleanup_status,
        ExecutionCleanupStatus::Cleaned
    );
    assert!(
        !tree_dir.exists(),
        "failed assembly should clean prepared tree '{}'",
        tree_dir.display()
    );
    assert!(
        !Path::new(&spec.workspace.out_dir)
            .join(".gaia/runtime/image-assembly.state")
            .exists()
    );
}

#[cfg(unix)]
#[test]
fn image_assembly_cleanup_failure_preserves_original_error() {
    use std::os::unix::fs::PermissionsExt;

    let mut spec = test_spec();
    let build_dir = Path::new(&spec.workspace.build_dir);
    let protected_parent = build_dir.join("protected-assembly");
    let tree_dir = protected_parent.join("boot");
    fs::create_dir_all(&tree_dir).expect("tree dir");
    fs::write(tree_dir.join("stale"), "stale").expect("stale file");
    fs::set_permissions(&protected_parent, fs::Permissions::from_mode(0o555))
        .expect("protect parent");

    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some(protected_parent.display().to_string().into()),
        trees: vec![AssemblyTreeSpec {
            id: "boot".into(),
            path: "$assembly.work/boot".into(),
        }],
        files: vec![AssemblyFileSpec {
            tree: "boot".into(),
            src: Some(build_dir.join("missing.txt").display().to_string().into()),
            src_glob: None,
            dest: "missing.txt".into(),
            mode: None,
            optional: false,
            preserve_symlink: false,
        }],
        ..ImageAssemblySpec::default()
    });
    let plan = gaia_plan::ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![PlannedOperation::new(
            OperationId::image_assembly(),
            OperationKind::AssembleImage,
        )],
    };
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );

    fs::set_permissions(&protected_parent, fs::Permissions::from_mode(0o755))
        .expect("restore parent");
    let _ = fs::remove_dir_all(&protected_parent);

    assert_eq!(outcome.errors.len(), 1);
    assert_eq!(outcome.errors[0].code, "assembly_execution_failed");
    assert!(
        outcome.errors[0]
            .message
            .contains("failed to clean assembly tree")
            || outcome.errors[0].message.contains("assembly source")
    );
    assert_eq!(
        outcome.errors[0].cleanup_status,
        ExecutionCleanupStatus::Failed
    );
    assert!(
        !outcome.errors[0].cleanup_failures.is_empty(),
        "cleanup failure should be attached without replacing the original error"
    );
    assert!(!outcome.cleanup_failures.is_empty());
    assert!(
        outcome.events.iter().any(|event| {
            matches!(
                event,
                gaia_exec::ExecutionEvent::Log { message, .. }
                    if message.contains("cleanup failure:")
            )
        }),
        "cleanup failure should be visible in runtime events"
    );
}

#[test]
fn image_assembly_transform_failure_cleans_prior_outputs() {
    let mut spec = test_spec();
    let build_dir = Path::new(&spec.workspace.build_dir);
    let source_dir = build_dir.join("transform-cleanup-sources");
    let output_dir = build_dir.join("transform-cleanup-output");
    fs::create_dir_all(&source_dir).expect("source dir");
    fs::write(source_dir.join("license.txt"), "license").expect("source file");

    spec.image.assembly = Some(ImageAssemblySpec {
        transforms: vec![
            AssemblyTransformSpec {
                kind: AssemblyTransformKindSpec::Copy,
                src: Some(source_dir.join("license.txt").display().to_string().into()),
                dest: output_dir.join("license.txt").display().to_string().into(),
                deterministic: true,
            },
            AssemblyTransformSpec {
                kind: AssemblyTransformKindSpec::Copy,
                src: Some(source_dir.join("missing.txt").display().to_string().into()),
                dest: output_dir.join("missing.txt").display().to_string().into(),
                deterministic: true,
            },
        ],
        ..ImageAssemblySpec::default()
    });
    let plan = gaia_plan::ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![PlannedOperation::new(
            OperationId::image_assembly(),
            OperationKind::AssembleImage,
        )],
    };
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );

    assert_eq!(outcome.errors.len(), 1);
    assert_eq!(outcome.errors[0].code, "assembly_execution_failed");
    assert_eq!(
        outcome.errors[0].cleanup_status,
        ExecutionCleanupStatus::Cleaned
    );
    assert!(!output_dir.join("license.txt").exists());
    assert!(!output_dir.join("missing.txt").exists());
}

#[test]
fn image_assembly_disk_failure_cleans_generated_filesystem_output() {
    let mut spec = test_spec();
    let build_dir = Path::new(&spec.workspace.build_dir);
    let source_dir = build_dir.join("disk-cleanup-sources");
    let tree_dir = build_dir.join("disk-cleanup-tree");
    let output_dir = build_dir.join("disk-cleanup-output");
    fs::create_dir_all(&source_dir).expect("source dir");
    fs::write(source_dir.join("init"), "init").expect("source file");

    spec.image.assembly = Some(ImageAssemblySpec {
        trees: vec![AssemblyTreeSpec {
            id: "initramfs".into(),
            path: tree_dir.display().to_string().into(),
        }],
        files: vec![AssemblyFileSpec {
            tree: "initramfs".into(),
            src: Some(source_dir.join("init").display().to_string().into()),
            src_glob: None,
            dest: "init".into(),
            mode: None,
            optional: false,
            preserve_symlink: false,
        }],
        filesystems: vec![AssemblyFilesystemSpec {
            id: "initramfs".into(),
            kind: AssemblyFilesystemKindSpec::Cpio,
            source_tree: "initramfs".into(),
            output: output_dir
                .join("initramfs.cpio")
                .display()
                .to_string()
                .into(),
            size: None,
            deterministic: true,
        }],
        disks: vec![AssemblyDiskSpec {
            id: "sdcard".into(),
            output: output_dir.join("sdcard.img").display().to_string().into(),
            partition_table: AssemblyPartitionTableSpec::Mbr,
            signature: None,
            signature_text: None,
            first_lba: None,
            alignment_lba: None,
            partitions: vec![AssemblyDiskPartitionSpec {
                name: "missing".into(),
                kind: None,
                type_alias: Some("linux".into()),
                bootable: false,
                image: output_dir
                    .join("missing-rootfs.img")
                    .display()
                    .to_string()
                    .into(),
            }],
        }],
        ..ImageAssemblySpec::default()
    });
    let plan = gaia_plan::ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![PlannedOperation::new(
            OperationId::image_assembly(),
            OperationKind::AssembleImage,
        )],
    };
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );

    assert_eq!(outcome.errors.len(), 1);
    assert_eq!(outcome.errors[0].code, "assembly_execution_failed");
    assert_eq!(
        outcome.errors[0].cleanup_status,
        ExecutionCleanupStatus::Cleaned
    );
    assert!(!tree_dir.exists());
    assert!(!output_dir.join("initramfs.cpio").exists());
    assert!(!output_dir.join("sdcard.img").exists());
}

#[cfg(unix)]
#[test]
fn image_assembly_disk_write_failure_cleans_temp_and_final_outputs() {
    let mut spec = test_spec();
    let build_dir = Path::new(&spec.workspace.build_dir);
    let source_dir = build_dir.join("disk-write-cleanup-sources");
    let output_dir = build_dir.join("disk-write-cleanup-output");
    fs::create_dir_all(&source_dir).expect("source dir");
    fs::create_dir_all(source_dir.join("rootfs-directory")).expect("partition directory");

    spec.image.assembly = Some(ImageAssemblySpec {
        disks: vec![AssemblyDiskSpec {
            id: "sdcard".into(),
            output: output_dir.join("sdcard.img").display().to_string().into(),
            partition_table: AssemblyPartitionTableSpec::Mbr,
            signature: None,
            signature_text: None,
            first_lba: None,
            alignment_lba: None,
            partitions: vec![AssemblyDiskPartitionSpec {
                name: "rootfs".into(),
                kind: None,
                type_alias: Some("linux".into()),
                bootable: false,
                image: source_dir
                    .join("rootfs-directory")
                    .display()
                    .to_string()
                    .into(),
            }],
        }],
        ..ImageAssemblySpec::default()
    });
    let plan = gaia_plan::ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![PlannedOperation::new(
            OperationId::image_assembly(),
            OperationKind::AssembleImage,
        )],
    };
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );

    assert_eq!(outcome.errors.len(), 1);
    assert_eq!(outcome.errors[0].code, "assembly_execution_failed");
    assert_eq!(
        outcome.errors[0].cleanup_status,
        ExecutionCleanupStatus::Cleaned
    );
    assert!(!output_dir.join("sdcard.img").exists());
    assert!(!output_dir.join(".sdcard.img.gaia-tmp").exists());
}

#[cfg(unix)]
#[test]
fn image_assembly_filesystem_failure_cleans_temp_and_final_outputs() {
    use std::os::unix::fs::PermissionsExt;

    let mut spec = test_spec();
    let build_dir = Path::new(&spec.workspace.build_dir);
    let source_dir = build_dir.join("filesystem-cleanup-sources");
    let tree_dir = build_dir.join("filesystem-cleanup-tree");
    let output_dir = build_dir.join("filesystem-cleanup-output");
    let host_bin = build_dir.join("image/buildroot-output/host/bin");
    fs::create_dir_all(&source_dir).expect("source dir");
    fs::create_dir_all(&host_bin).expect("host bin");
    fs::write(source_dir.join("boot.txt"), "boot").expect("source file");
    let mformat = host_bin.join("mformat");
    let mcopy = host_bin.join("mcopy");
    fs::write(
        &mformat,
        "#!/usr/bin/env sh\necho mformat failed >&2\nexit 1\n",
    )
    .expect("mformat");
    fs::write(&mcopy, "#!/usr/bin/env sh\nexit 0\n").expect("mcopy");
    fs::set_permissions(&mformat, fs::Permissions::from_mode(0o755)).expect("mformat mode");
    fs::set_permissions(&mcopy, fs::Permissions::from_mode(0o755)).expect("mcopy mode");

    spec.image.assembly = Some(ImageAssemblySpec {
        trees: vec![AssemblyTreeSpec {
            id: "boot".into(),
            path: tree_dir.display().to_string().into(),
        }],
        files: vec![AssemblyFileSpec {
            tree: "boot".into(),
            src: Some(source_dir.join("boot.txt").display().to_string().into()),
            src_glob: None,
            dest: "boot.txt".into(),
            mode: None,
            optional: false,
            preserve_symlink: false,
        }],
        filesystems: vec![AssemblyFilesystemSpec {
            id: "bootfs".into(),
            kind: AssemblyFilesystemKindSpec::Vfat,
            source_tree: "boot".into(),
            output: output_dir.join("boot.vfat").display().to_string().into(),
            size: Some("1M".into()),
            deterministic: false,
        }],
        ..ImageAssemblySpec::default()
    });
    let plan = gaia_plan::ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![PlannedOperation::new(
            OperationId::image_assembly(),
            OperationKind::AssembleImage,
        )],
    };
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );

    assert_eq!(outcome.errors.len(), 1);
    assert_eq!(outcome.errors[0].code, "assembly_execution_failed");
    assert_eq!(
        outcome.errors[0].cleanup_status,
        ExecutionCleanupStatus::Cleaned
    );
    assert!(!tree_dir.exists());
    assert!(!output_dir.join("boot.vfat").exists());
    assert!(!output_dir.join(".boot.vfat.gaia-tmp").exists());
}
