pub mod support;

use gaia_exec::{
    ExecutionCancellation, ExecutionErrorKind, ExecutionProviders, execute_plan,
    execute_plan_with_cancellation,
};
use gaia_plan::{OperationId, OperationKind, PlannedOperation};
use gaia_spec::{
    AssemblyBusyboxInitramfsSpec, AssemblyDirSpec, AssemblyFileSpec, AssemblyFilesystemKindSpec,
    AssemblySymlinkSpec, AssemblyTransformKindSpec, AssemblyTransformSpec, ImageAssemblySpec,
};
use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};
use support::{
    assembly_file, assembly_filesystem, assembly_transform, assembly_tree, provider_catalogs,
    test_spec,
};

#[test]
fn executes_image_assembly_copy_and_gzip_transforms() {
    let mut spec = test_spec();
    let build_dir = Path::new(&spec.workspace.build_dir);
    let source_dir = build_dir.join("transform-sources");
    let output_dir = build_dir.join("transform-output");
    fs::create_dir_all(&source_dir).expect("transform source dir");
    fs::write(source_dir.join("kernel"), "kernel bytes").expect("kernel source");
    fs::write(source_dir.join("license.txt"), "license").expect("license source");

    spec.image.assembly = Some(ImageAssemblySpec {
        transforms: vec![
            assembly_transform(
                AssemblyTransformKindSpec::Copy,
                source_dir.join("license.txt").display(),
                output_dir.join("license.txt").display(),
            ),
            assembly_transform(
                AssemblyTransformKindSpec::Gzip,
                source_dir.join("kernel").display(),
                output_dir.join("kernel.img").display(),
            ),
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
    let first = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    let first_gzip = fs::read(output_dir.join("kernel.img")).expect("gzip output");
    assert_eq!(
        fs::read_to_string(output_dir.join("license.txt")).unwrap(),
        "license"
    );
    assert_eq!(&first_gzip[..2], &[0x1f, 0x8b]);

    let second = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );
    assert!(second.errors.is_empty(), "{:?}", second.errors);
    assert_eq!(
        fs::read(output_dir.join("kernel.img")).expect("second gzip output"),
        first_gzip
    );
    let state = fs::read_to_string(
        Path::new(&spec.workspace.out_dir).join(".gaia/runtime/image-assembly.state"),
    )
    .expect("assembly state");
    assert!(state.contains("completed_transform_count=2"));
    assert!(state.contains("transform.1.kind=copy"));
    assert!(state.contains("transform.1.deterministic=true"));
    assert!(state.contains("transform.2.kind=gzip"));
    assert!(state.contains("transform.2.deterministic=true"));
    assert!(state.contains("transform.2.tool="));
    assert!(state.contains("transform.2.tool_version=gzip"));
    assert!(state.contains("transform.2.sha256="));
}

#[cfg(unix)]
#[test]
fn gzip_transform_failure_reports_bounded_stderr_tail_and_tool_path() {
    use std::os::unix::fs::PermissionsExt;

    let mut spec = test_spec();
    spec.policy.execution.output_retention.stderr_bytes = 4;
    let collect_dir = spec.image.output.collect_dir.clone().expect("collect dir");
    let provider_bin = Path::new(&collect_dir).join("buildroot-output/host/bin");
    fs::create_dir_all(&provider_bin).expect("provider bin");
    let fake_gzip = provider_bin.join("gzip");
    fs::write(&fake_gzip, "#!/bin/sh\nprintf 'prefixTAIL' >&2\nexit 7\n").expect("fake gzip");
    let mut permissions = fs::metadata(&fake_gzip)
        .expect("fake gzip metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_gzip, permissions).expect("fake gzip executable");

    let source_dir = Path::new(&spec.workspace.build_dir).join("gzip-fail-source");
    let output_dir = Path::new(&spec.workspace.build_dir).join("gzip-fail-output");
    fs::create_dir_all(&source_dir).expect("source dir");
    fs::write(source_dir.join("kernel"), "kernel").expect("source file");
    spec.image.assembly = Some(ImageAssemblySpec {
        transforms: vec![AssemblyTransformSpec {
            kind: AssemblyTransformKindSpec::Gzip,
            src: Some(source_dir.join("kernel").display().to_string().into()),
            dest: output_dir.join("kernel.img").display().to_string().into(),
            deterministic: true,
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
    let message = &outcome.errors[0].message;
    assert!(
        message.contains(&fake_gzip.display().to_string()),
        "{message}"
    );
    assert!(message.contains("exit status: 7"), "{message}");
    assert!(message.contains("stderr tail: TAIL"), "{message}");
    assert!(!message.contains("prefix"), "{message}");
}

#[cfg(unix)]
#[test]
fn gzip_transform_tool_start_failure_is_typed() {
    let mut spec = test_spec();
    let collect_dir = spec.image.output.collect_dir.clone().expect("collect dir");
    let provider_bin = Path::new(&collect_dir).join("buildroot-output/host/bin");
    fs::create_dir_all(&provider_bin).expect("provider bin");
    let fake_gzip = provider_bin.join("gzip");
    fs::write(&fake_gzip, "#!/bin/sh\nprintf unused\n").expect("fake gzip");

    let source_dir = Path::new(&spec.workspace.build_dir).join("gzip-tool-start-source");
    let output_dir = Path::new(&spec.workspace.build_dir).join("gzip-tool-start-output");
    fs::create_dir_all(&source_dir).expect("source dir");
    fs::write(source_dir.join("kernel"), "kernel").expect("source file");
    spec.image.assembly = Some(ImageAssemblySpec {
        transforms: vec![AssemblyTransformSpec {
            kind: AssemblyTransformKindSpec::Gzip,
            src: Some(source_dir.join("kernel").display().to_string().into()),
            dest: output_dir.join("kernel.img").display().to_string().into(),
            deterministic: true,
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
    assert_eq!(outcome.errors[0].kind, ExecutionErrorKind::ToolStart);
    assert!(
        outcome.errors[0].message.contains("failed to start"),
        "{}",
        outcome.errors[0].message
    );
}

#[cfg(unix)]
#[test]
fn gzip_transform_honors_assembly_command_timeout() {
    use std::os::unix::fs::PermissionsExt;

    let mut spec = test_spec();
    spec.policy.providers.buildroot.timeout_seconds = 1;
    let collect_dir = spec.image.output.collect_dir.clone().expect("collect dir");
    let provider_bin = Path::new(&collect_dir).join("buildroot-output/host/bin");
    fs::create_dir_all(&provider_bin).expect("provider bin");
    let fake_gzip = provider_bin.join("gzip");
    fs::write(&fake_gzip, "#!/bin/sh\nsleep 10\n").expect("fake gzip");
    let mut permissions = fs::metadata(&fake_gzip)
        .expect("fake gzip metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_gzip, permissions).expect("fake gzip executable");

    let source_dir = Path::new(&spec.workspace.build_dir).join("gzip-timeout-source");
    let output_dir = Path::new(&spec.workspace.build_dir).join("gzip-timeout-output");
    fs::create_dir_all(&source_dir).expect("source dir");
    fs::write(source_dir.join("kernel"), "kernel").expect("source file");
    spec.image.assembly = Some(ImageAssemblySpec {
        transforms: vec![AssemblyTransformSpec {
            kind: AssemblyTransformKindSpec::Gzip,
            src: Some(source_dir.join("kernel").display().to_string().into()),
            dest: output_dir.join("kernel.img").display().to_string().into(),
            deterministic: true,
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

#[cfg(unix)]
#[test]
fn gzip_transform_succeeds_when_tool_version_probe_hangs() {
    use std::os::unix::fs::PermissionsExt;

    let mut spec = test_spec();
    spec.policy.providers.buildroot.timeout_seconds = 10;
    let collect_dir = spec.image.output.collect_dir.clone().expect("collect dir");
    let provider_bin = Path::new(&collect_dir).join("buildroot-output/host/bin");
    fs::create_dir_all(&provider_bin).expect("provider bin");
    let fake_gzip = provider_bin.join("gzip");
    fs::write(
        &fake_gzip,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then sleep 30; exit 0; fi\nprintf compressed\n",
    )
    .expect("fake gzip");
    let mut permissions = fs::metadata(&fake_gzip)
        .expect("fake gzip metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_gzip, permissions).expect("fake gzip executable");

    let source_dir = Path::new(&spec.workspace.build_dir).join("gzip-version-hang-source");
    let output_dir = Path::new(&spec.workspace.build_dir).join("gzip-version-hang-output");
    let dest = output_dir.join("kernel.img");
    fs::create_dir_all(&source_dir).expect("source dir");
    fs::write(source_dir.join("kernel"), "kernel").expect("source file");
    spec.image.assembly = Some(ImageAssemblySpec {
        transforms: vec![AssemblyTransformSpec {
            kind: AssemblyTransformKindSpec::Gzip,
            src: Some(source_dir.join("kernel").display().to_string().into()),
            dest: dest.display().to_string().into(),
            deterministic: true,
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

    let started = Instant::now();
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
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "hanging version probe should be bounded"
    );
    assert_eq!(
        fs::read_to_string(&dest).expect("gzip output"),
        "compressed"
    );
    let state = fs::read_to_string(
        Path::new(&spec.workspace.out_dir).join(".gaia/runtime/image-assembly.state"),
    )
    .expect("assembly state");
    assert!(state.contains(&format!("transform.1.tool={}", fake_gzip.display())));
    assert!(!state.contains("transform.1.tool_version="));
}

#[cfg(unix)]
#[test]
fn gzip_transform_honors_execution_cancellation() {
    use std::os::unix::fs::PermissionsExt;

    let mut spec = test_spec();
    spec.policy.providers.buildroot.timeout_seconds = 30;
    let collect_dir = spec.image.output.collect_dir.clone().expect("collect dir");
    let provider_bin = Path::new(&collect_dir).join("buildroot-output/host/bin");
    fs::create_dir_all(&provider_bin).expect("provider bin");
    let fake_gzip = provider_bin.join("gzip");
    fs::write(&fake_gzip, "#!/bin/sh\nsleep 10\n").expect("fake gzip");
    let mut permissions = fs::metadata(&fake_gzip)
        .expect("fake gzip metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_gzip, permissions).expect("fake gzip executable");

    let source_dir = Path::new(&spec.workspace.build_dir).join("gzip-cancel-source");
    let output_dir = Path::new(&spec.workspace.build_dir).join("gzip-cancel-output");
    fs::create_dir_all(&source_dir).expect("source dir");
    fs::write(source_dir.join("kernel"), "kernel").expect("source file");
    spec.image.assembly = Some(ImageAssemblySpec {
        transforms: vec![AssemblyTransformSpec {
            kind: AssemblyTransformKindSpec::Gzip,
            src: Some(source_dir.join("kernel").display().to_string().into()),
            dest: output_dir.join("kernel.img").display().to_string().into(),
            deterministic: true,
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
    let cancellation = ExecutionCancellation::new();
    let cancellation_trigger = cancellation.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        cancellation_trigger.cancel();
    });
    let outcome = execute_plan_with_cancellation(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
        &cancellation,
    );

    assert_eq!(outcome.errors.len(), 1);
    assert_eq!(outcome.errors[0].kind, ExecutionErrorKind::Cancelled);
    assert!(
        outcome.errors[0].message.contains("cancelled"),
        "{}",
        outcome.errors[0].message
    );
}

#[cfg(unix)]
#[test]
fn compile_dts_prefers_provider_host_dtc() {
    use std::os::unix::fs::PermissionsExt;

    let mut spec = test_spec();
    let collect_dir = Path::new(&spec.workspace.out_dir).join("images");
    spec.image.output.collect_dir = Some(collect_dir.display().to_string());
    let provider_bin = collect_dir.join("buildroot-output/host/bin");
    fs::create_dir_all(&provider_bin).expect("provider bin");
    let fake_dtc = provider_bin.join("dtc");
    fs::write(
        &fake_dtc,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'fake dtc 1.0'; exit 0; fi\nout=''\nwhile [ \"$#\" -gt 0 ]; do if [ \"$1\" = '-o' ]; then shift; out=\"$1\"; fi; shift; done\nprintf 'compiled dtbo' > \"$out\"\n",
    )
    .expect("fake dtc");
    let mut permissions = fs::metadata(&fake_dtc)
        .expect("fake dtc metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_dtc, permissions).expect("fake dtc executable");

    let source_dir = Path::new(&spec.workspace.build_dir).join("dts");
    fs::create_dir_all(&source_dir).expect("dts dir");
    fs::write(source_dir.join("overlay.dts"), "/dts-v1/;\n/plugin/;\n").expect("dts source");
    let dest = Path::new(&spec.workspace.build_dir).join("assembly/boot/overlay.dtbo");
    spec.image.assembly = Some(ImageAssemblySpec {
        transforms: vec![AssemblyTransformSpec {
            kind: AssemblyTransformKindSpec::CompileDts,
            src: Some(source_dir.join("overlay.dts").display().to_string().into()),
            dest: dest.display().to_string().into(),
            deterministic: true,
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
    assert_eq!(fs::read_to_string(&dest).expect("dtbo"), "compiled dtbo");
    let state = fs::read_to_string(
        Path::new(&spec.workspace.out_dir).join(".gaia/runtime/image-assembly.state"),
    )
    .expect("assembly state");
    assert!(state.contains("transform.1.kind=compile-dts"));
    assert!(state.contains(&format!("transform.1.tool={}", fake_dtc.display())));
    assert!(state.contains("transform.1.tool_version=fake dtc 1.0"));
}

#[test]
fn executes_image_assembly_cpio_filesystems() {
    let mut spec = test_spec();
    let build_dir = Path::new(&spec.workspace.build_dir);
    let source_dir = build_dir.join("filesystem-sources");
    let output_dir = build_dir.join("filesystem-output");
    fs::create_dir_all(&source_dir).expect("filesystem source dir");
    fs::write(source_dir.join("init"), "#!/bin/sh\n").expect("init source");

    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some(build_dir.join("assembly").display().to_string().into()),
        trees: vec![assembly_tree("initramfs", "$assembly.work/initramfs")],
        files: vec![AssemblyFileSpec {
            mode: Some("0755".into()),
            ..assembly_file("initramfs", source_dir.join("init").display(), "init")
        }],
        filesystems: vec![
            assembly_filesystem(
                "initramfs-cpio",
                AssemblyFilesystemKindSpec::Cpio,
                "initramfs",
                output_dir.join("initramfs.cpio").display(),
            ),
            assembly_filesystem(
                "initramfs-gz",
                AssemblyFilesystemKindSpec::CpioGzip,
                "initramfs",
                output_dir.join("initramfs.cpio.gz").display(),
            ),
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
    let first = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    let cpio = fs::read(output_dir.join("initramfs.cpio")).expect("cpio output");
    let gzip = fs::read(output_dir.join("initramfs.cpio.gz")).expect("gzip cpio output");
    assert!(cpio.starts_with(b"070701"));
    assert!(
        cpio.windows("init\0".len())
            .any(|window| window == b"init\0")
    );
    assert_eq!(&gzip[..2], &[0x1f, 0x8b]);

    let second = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );
    assert!(second.errors.is_empty(), "{:?}", second.errors);
    assert_eq!(
        fs::read(output_dir.join("initramfs.cpio")).expect("second cpio"),
        cpio
    );
    assert_eq!(
        fs::read(output_dir.join("initramfs.cpio.gz")).expect("second gzip cpio"),
        gzip
    );

    let state = fs::read_to_string(
        Path::new(&spec.workspace.out_dir).join(".gaia/runtime/image-assembly.state"),
    )
    .expect("assembly state");
    assert!(state.contains("completed_filesystem_count=2"));
    assert!(state.contains("filesystem.1.kind=cpio"));
    assert!(state.contains("filesystem.1.deterministic=true"));
    assert!(state.contains("filesystem.2.kind=cpio-gzip"));
    assert!(state.contains("filesystem.2.deterministic=true"));
    assert!(state.contains("filesystem.2.tool="));
    assert!(state.contains("filesystem.2.sha256="));
}

#[cfg(unix)]
#[test]
fn executes_busybox_initramfs_and_packs_cpio_gzip() {
    use std::os::unix::fs::PermissionsExt;

    let mut spec = test_spec();
    let build_dir = Path::new(&spec.workspace.build_dir);
    let source_dir = build_dir.join("busybox-sources");
    let output_dir = build_dir.join("busybox-output");
    fs::create_dir_all(&source_dir).expect("busybox source dir");
    let busybox = source_dir.join("busybox");
    fs::write(&busybox, "#!/bin/sh\n").expect("busybox source");
    let mut permissions = fs::metadata(&busybox)
        .expect("busybox metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&busybox, permissions).expect("busybox executable");

    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some(build_dir.join("assembly").display().to_string().into()),
        trees: vec![assembly_tree("initramfs", "$assembly.work/initramfs")],
        dirs: vec![
            AssemblyDirSpec {
                tree: "initramfs".into(),
                path: "dev".into(),
                mode: None,
            },
            AssemblyDirSpec {
                tree: "initramfs".into(),
                path: "mnt/lower".into(),
                mode: Some("0755".into()),
            },
        ],
        symlinks: vec![
            AssemblySymlinkSpec {
                tree: "initramfs".into(),
                path: "lib64".into(),
                target: "lib".into(),
            },
            AssemblySymlinkSpec {
                tree: "initramfs".into(),
                path: "usr/lib64".into(),
                target: "../lib".into(),
            },
        ],
        busybox_initramfs: vec![AssemblyBusyboxInitramfsSpec {
            tree: "initramfs".into(),
            busybox: busybox.display().to_string().into(),
            include_runtime_libs: false,
            applets: vec!["sh".into(), "mount".into(), "insmod".into()],
        }],
        filesystems: vec![assembly_filesystem(
            "initramfs",
            AssemblyFilesystemKindSpec::CpioGzip,
            "initramfs",
            output_dir.join("initramfs.cpio.gz").display(),
        )],
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
    let tree = build_dir.join("assembly/initramfs");
    assert_eq!(
        fs::read_to_string(tree.join("bin/busybox")).expect("busybox"),
        "#!/bin/sh\n"
    );
    assert_eq!(
        fs::read_link(tree.join("bin/sh")).expect("sh applet"),
        Path::new("busybox")
    );
    assert_eq!(
        fs::read_link(tree.join("bin/mount")).expect("mount applet"),
        Path::new("busybox")
    );
    assert!(tree.join("dev").is_dir());
    assert!(tree.join("mnt/lower").is_dir());
    assert_eq!(
        fs::read_link(tree.join("lib64")).expect("lib64 symlink"),
        Path::new("lib")
    );
    assert_eq!(
        fs::read_link(tree.join("usr/lib64")).expect("usr/lib64 symlink"),
        Path::new("../lib")
    );
    assert_eq!(
        fs::read_link(tree.join("bin/insmod")).expect("insmod applet"),
        Path::new("busybox")
    );
    assert_eq!(
        &fs::read(output_dir.join("initramfs.cpio.gz")).unwrap()[..2],
        &[0x1f, 0x8b]
    );
    let state = fs::read_to_string(
        Path::new(&spec.workspace.out_dir).join(".gaia/runtime/image-assembly.state"),
    )
    .expect("assembly state");
    assert!(state.contains("created_dir_count=2"));
    assert!(state.contains("created_symlink_count=2"));
    assert!(state.contains("completed_busybox_initramfs_count=1"));
    assert!(state.contains("busybox.1.applet_count=3"));
    assert!(state.contains("busybox.1.runtime_linkage=not-requested"));
    assert!(state.contains("completed_filesystem_count=1"));
}
