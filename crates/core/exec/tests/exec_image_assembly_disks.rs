pub mod support;

use gaia_exec::{ExecutionErrorKind, ExecutionProviders, execute_plan};
use gaia_plan::{OperationId, OperationKind, PlannedOperation};
use gaia_spec::{
    AssemblyDiskPartitionSpec, AssemblyDiskSpec, AssemblyFileSpec, AssemblyFilesystemKindSpec,
    AssemblyFilesystemSpec, AssemblyPartitionTableSpec, AssemblyTreeSpec, ImageAssemblySpec,
};
use std::fs;
use std::path::Path;
use std::process::Command;
use support::{provider_catalogs, test_spec};

#[cfg(unix)]
#[test]
fn executes_image_assembly_vfat_filesystem_with_provider_mtools() {
    use std::os::unix::fs::PermissionsExt;

    let mut spec = test_spec();
    let build_dir = Path::new(&spec.workspace.build_dir);
    let output_dir = build_dir.join("filesystem-output");
    let collect_dir = Path::new(&spec.workspace.out_dir).join("images");
    spec.image.output.collect_dir = Some(collect_dir.display().to_string());
    let provider_bin = collect_dir.join("buildroot-output/host/bin");
    fs::create_dir_all(&provider_bin).expect("provider bin");
    let log = build_dir.join("mtools.log");
    let fake_mformat = provider_bin.join("mformat");
    let fake_mcopy = provider_bin.join("mcopy");
    fs::write(
        &fake_mformat,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'fake mformat 1.0'; exit 0; fi\necho mformat \"$@\" >> '{}'\nexit 0\n",
            log.display()
        ),
    )
    .expect("fake mformat");
    fs::write(
        &fake_mcopy,
        format!(
            "#!/bin/sh\necho mcopy \"$@\" >> '{}'\nexit 0\n",
            log.display()
        ),
    )
    .expect("fake mcopy");
    for tool in [&fake_mformat, &fake_mcopy] {
        let mut permissions = fs::metadata(tool).expect("tool metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(tool, permissions).expect("tool executable");
    }

    let source_dir = build_dir.join("filesystem-sources");
    fs::create_dir_all(&source_dir).expect("filesystem source dir");
    fs::write(source_dir.join("z.txt"), "z").expect("z source");
    fs::write(source_dir.join("a.txt"), "a").expect("a source");
    let rootfs_image = build_dir.join("rootfs.squashfs");
    fs::write(&rootfs_image, vec![0x33; 1024]).expect("rootfs image");
    let boot_vfat = output_dir.join("boot.vfat");
    let sdcard = output_dir.join("sdcard.img");
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some(build_dir.join("assembly").display().to_string().into()),
        trees: vec![AssemblyTreeSpec {
            id: "boot".into(),
            path: "$assembly.work/boot".into(),
        }],
        files: vec![AssemblyFileSpec {
            tree: "boot".into(),
            src_glob: Some(source_dir.join("*.txt").display().to_string().into()),
            src: None,
            dest: ".".into(),
            mode: None,
            optional: false,
            preserve_symlink: false,
        }],
        filesystems: vec![AssemblyFilesystemSpec {
            id: "bootfs".into(),
            kind: AssemblyFilesystemKindSpec::Vfat,
            source_tree: "boot".into(),
            output: boot_vfat.display().to_string().into(),
            size: Some("1M".into()),
            deterministic: false,
        }],
        disks: vec![AssemblyDiskSpec {
            id: "sdcard".into(),
            output: sdcard.display().to_string().into(),
            partition_table: AssemblyPartitionTableSpec::Mbr,
            signature: Some("0x48454c49".into()),
            signature_text: None,
            partitions: vec![
                AssemblyDiskPartitionSpec {
                    name: "boot".into(),
                    kind: None,
                    type_alias: Some("fat32-lba".into()),
                    bootable: true,
                    image: boot_vfat.display().to_string().into(),
                },
                AssemblyDiskPartitionSpec {
                    name: "rootfs".into(),
                    kind: Some("0x83".into()),
                    type_alias: None,
                    bootable: false,
                    image: rootfs_image.display().to_string().into(),
                },
            ],
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

    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    assert_eq!(
        fs::metadata(&boot_vfat).expect("vfat output").len(),
        1024 * 1024
    );
    let disk = fs::read(&sdcard).expect("sdcard output");
    assert_eq!(&disk[510..512], &[0x55, 0xaa]);
    assert_eq!(disk[450], 0x0c);
    assert_eq!(disk[466], 0x83);
    let log = fs::read_to_string(log).expect("mtools log");
    assert!(log.contains("mformat -i"));
    let first_copy = log.find("a.txt").expect("a copied");
    let second_copy = log.find("z.txt").expect("z copied");
    assert!(first_copy < second_copy, "{log}");
    assert!(
        !log.contains(" -F "),
        "vfat assembly should let mformat choose a valid FAT variant for the image size: {log}"
    );
    let state = fs::read_to_string(
        Path::new(&spec.workspace.out_dir).join(".gaia/runtime/image-assembly.state"),
    )
    .expect("assembly state");
    assert!(state.contains("completed_filesystem_count=1"));
    assert!(state.contains("filesystem.1.kind=vfat"));
    assert!(state.contains("filesystem.1.deterministic=false"));
    assert!(state.contains(&format!("mformat={}", fake_mformat.display())));
    assert!(state.contains(&format!("mcopy={}", fake_mcopy.display())));
    assert!(state.contains("filesystem.1.tool_version=fake mformat 1.0"));
    assert!(state.contains("completed_disk_count=1"));
    assert!(state.contains("disk.1.partition.1.type=0x0C"));
    assert!(state.contains("disk.1.partition.2.type=0x83"));
}

#[test]
fn executes_image_assembly_archives_single_disk_as_raw_xz() {
    let mut spec = test_spec();
    let build_dir = Path::new(&spec.workspace.build_dir);
    let collect_dir = Path::new(&spec.workspace.out_dir).join("images");
    spec.image.output.collect_dir = Some(collect_dir.display().to_string());
    spec.image.output.archive_name = Some("published.img.xz".into());
    let boot_image = build_dir.join("boot.img");
    let rootfs_image = build_dir.join("rootfs.img");
    fs::create_dir_all(build_dir).expect("build dir");
    fs::write(&boot_image, vec![0x11; 1024]).expect("boot image");
    fs::write(&rootfs_image, vec![0x22; 2048]).expect("rootfs image");
    let sdcard = collect_dir.join("sdcard.img");
    spec.image.assembly = Some(ImageAssemblySpec {
        disks: vec![AssemblyDiskSpec {
            id: "sdcard".into(),
            output: sdcard.display().to_string().into(),
            partition_table: AssemblyPartitionTableSpec::Mbr,
            signature: None,
            signature_text: Some("GAIA".into()),
            partitions: vec![
                AssemblyDiskPartitionSpec {
                    name: "boot".into(),
                    kind: None,
                    type_alias: Some("fat32-lba".into()),
                    bootable: true,
                    image: boot_image.display().to_string().into(),
                },
                AssemblyDiskPartitionSpec {
                    name: "rootfs".into(),
                    kind: Some("0x83".into()),
                    type_alias: None,
                    bootable: false,
                    image: rootfs_image.display().to_string().into(),
                },
            ],
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

    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    let archive = collect_dir.join("published.img.xz");
    assert!(archive.is_file(), "missing archive {}", archive.display());
    let decompressed = Command::new("xz")
        .arg("-dc")
        .arg(&archive)
        .output()
        .expect("xz decompress");
    assert!(
        decompressed.status.success(),
        "{}",
        String::from_utf8_lossy(&decompressed.stderr)
    );
    assert_eq!(
        decompressed.stdout,
        fs::read(&sdcard).expect("sdcard output")
    );
    let state = fs::read_to_string(
        Path::new(&spec.workspace.out_dir).join(".gaia/runtime/image-assembly.state"),
    )
    .expect("assembly state");
    assert!(state.contains(&format!("archive.path={}", archive.display())));
    assert!(state.contains(&format!("archive.source={}", sdcard.display())));
}

#[cfg(unix)]
#[test]
fn vfat_filesystem_honors_mformat_timeout() {
    use std::os::unix::fs::PermissionsExt;

    let mut spec = test_spec();
    spec.policy.providers.buildroot.timeout_seconds = 1;
    let build_dir = Path::new(&spec.workspace.build_dir);
    let output_dir = build_dir.join("filesystem-timeout-output");
    let collect_dir = Path::new(&spec.workspace.out_dir).join("images");
    spec.image.output.collect_dir = Some(collect_dir.display().to_string());
    let provider_bin = collect_dir.join("buildroot-output/host/bin");
    fs::create_dir_all(&provider_bin).expect("provider bin");
    let fake_mformat = provider_bin.join("mformat");
    let fake_mcopy = provider_bin.join("mcopy");
    fs::write(&fake_mformat, "#!/bin/sh\nsleep 10\n").expect("fake mformat");
    fs::write(&fake_mcopy, "#!/bin/sh\nexit 0\n").expect("fake mcopy");
    for tool in [&fake_mformat, &fake_mcopy] {
        let mut permissions = fs::metadata(tool).expect("tool metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(tool, permissions).expect("tool executable");
    }

    let source_dir = build_dir.join("filesystem-timeout-sources");
    fs::create_dir_all(&source_dir).expect("filesystem source dir");
    fs::write(source_dir.join("boot.txt"), "boot").expect("boot source");
    let boot_vfat = output_dir.join("boot.vfat");
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some(
            build_dir
                .join("assembly-timeout")
                .display()
                .to_string()
                .into(),
        ),
        trees: vec![AssemblyTreeSpec {
            id: "boot".into(),
            path: "$assembly.work/boot".into(),
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
            output: boot_vfat.display().to_string().into(),
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
    assert_eq!(outcome.errors[0].kind, ExecutionErrorKind::Timeout);
    assert!(
        outcome.errors[0].message.contains("timed out after 1s"),
        "{}",
        outcome.errors[0].message
    );
}

#[test]
fn executes_image_assembly_mbr_disk() {
    let mut spec = test_spec();
    let build_dir = Path::new(&spec.workspace.build_dir);
    let image_dir = build_dir.join("disk-images");
    let output_dir = build_dir.join("disk-output");
    fs::create_dir_all(&image_dir).expect("image dir");
    fs::write(image_dir.join("boot.vfat"), vec![0x11; 700]).expect("boot image");
    fs::write(image_dir.join("rootfs.squashfs"), vec![0x22; 1024]).expect("rootfs image");

    spec.image.assembly = Some(ImageAssemblySpec {
        disks: vec![AssemblyDiskSpec {
            id: "sdcard".into(),
            output: output_dir.join("sdcard.img").display().to_string().into(),
            partition_table: AssemblyPartitionTableSpec::Mbr,
            signature: Some("0x48454c49".into()),
            signature_text: None,
            partitions: vec![
                AssemblyDiskPartitionSpec {
                    name: "boot".into(),
                    kind: None,
                    type_alias: Some("fat32-lba".into()),
                    bootable: true,
                    image: image_dir.join("boot.vfat").display().to_string().into(),
                },
                AssemblyDiskPartitionSpec {
                    name: "rootfs".into(),
                    kind: Some("0x83".into()),
                    type_alias: None,
                    bootable: false,
                    image: image_dir
                        .join("rootfs.squashfs")
                        .display()
                        .to_string()
                        .into(),
                },
            ],
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

    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    let disk = fs::read(output_dir.join("sdcard.img")).expect("disk");
    assert_eq!(&disk[440..444], &[0x49, 0x4c, 0x45, 0x48]);
    assert_eq!(&disk[510..512], &[0x55, 0xaa]);
    assert_eq!(disk[446], 0x80);
    assert_eq!(disk[450], 0x0c);
    assert_eq!(
        u32::from_le_bytes(disk[454..458].try_into().expect("boot start")),
        2048
    );
    assert_eq!(
        u32::from_le_bytes(disk[458..462].try_into().expect("boot sectors")),
        2
    );
    assert_eq!(disk[462], 0x00);
    assert_eq!(disk[466], 0x83);
    assert_eq!(
        u32::from_le_bytes(disk[470..474].try_into().expect("root start")),
        4096
    );
    assert_eq!(
        u32::from_le_bytes(disk[474..478].try_into().expect("root sectors")),
        2
    );
    let boot_offset = 2048 * 512;
    let root_offset = 4096 * 512;
    assert_eq!(&disk[boot_offset..boot_offset + 4], &[0x11; 4]);
    assert_eq!(&disk[root_offset..root_offset + 4], &[0x22; 4]);

    let state = fs::read_to_string(
        Path::new(&spec.workspace.out_dir).join(".gaia/runtime/image-assembly.state"),
    )
    .expect("assembly state");
    assert!(state.contains("completed_disk_count=1"));
    assert!(state.contains("disk.1.partition_table=mbr"));
    assert!(state.contains("disk.1.partition.1.type=0x0C"));
    assert!(state.contains("disk.1.partition.1.start_lba=2048"));
    assert!(state.contains("disk.1.partition.2.start_lba=4096"));
    assert!(state.contains("disk.1.sha256="));
}

#[test]
fn image_assembly_mbr_rejects_more_than_four_partitions_before_writing_disk() {
    let mut spec = test_spec();
    let build_dir = Path::new(&spec.workspace.build_dir);
    let image_dir = build_dir.join("disk-images");
    let output = build_dir.join("disk-output/sdcard.img");
    fs::create_dir_all(&image_dir).expect("image dir");
    for index in 0..5 {
        fs::write(image_dir.join(format!("part{index}.img")), "partition").expect("partition");
    }

    spec.image.assembly = Some(ImageAssemblySpec {
        disks: vec![AssemblyDiskSpec {
            id: "sdcard".into(),
            output: output.display().to_string().into(),
            partition_table: AssemblyPartitionTableSpec::Mbr,
            signature: None,
            signature_text: None,
            partitions: (0..5)
                .map(|index| AssemblyDiskPartitionSpec {
                    name: format!("part{index}"),
                    kind: Some("0x83".into()),
                    type_alias: None,
                    bootable: false,
                    image: image_dir
                        .join(format!("part{index}.img"))
                        .display()
                        .to_string()
                        .into(),
                })
                .collect(),
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
    assert!(outcome.errors[0].message.contains("MBR supports at most 4"));
    assert!(!output.exists());
}
