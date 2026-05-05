use super::*;
use gaia_spec::expand_simple_glob;

use gaia_spec::{
    AssemblyFileSpec, AssemblyTreeSpec, ImageAssemblySpec, ImageOutputSpec, WorkspaceNamedPathSpec,
    WorkspacePathKindSpec,
};
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::time::Instant;
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_dir(prefix: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("{prefix}-{nonce}"));
    fs::create_dir_all(&root).expect("test root");
    root
}

fn test_spec(root: &Path) -> ResolvedBuildSpec {
    let mut spec = ResolvedBuildSpec::new("assembly-helper-test");
    spec.workspace.root_dir = root.display().to_string();
    spec.workspace.build_dir = root.join("build").display().to_string();
    spec.workspace.out_dir = root.join("out").display().to_string();
    spec.workspace.named_paths = vec![WorkspaceNamedPathSpec {
        alias: "assets".into(),
        path: root.join("assets").display().to_string(),
        kind: WorkspacePathKindSpec::Host,
    }];
    spec.image.output = ImageOutputSpec {
        collect_dir: Some(root.join("out/images").display().to_string()),
        archive_name: None,
        emit_report: true,
    };
    spec
}

#[test]
fn assembly_file_dest_maps_directory_dest_and_rejects_escape() {
    let root = unique_dir("gaia-assembly-dest");
    let tree = root.join("tree");
    let source = root.join("sources/config.txt");
    fs::create_dir_all(source.parent().expect("source parent")).expect("source parent");
    fs::write(&source, "config").expect("source");

    assert_eq!(
        assembly_file_dest(&tree, &source, ".").expect("directory dest"),
        tree.join("config.txt")
    );
    assert!(
        assembly_file_dest(&tree, &source, "../escape.txt")
            .expect_err("escape should fail")
            .contains("escapes tree")
    );
    assert!(
        assembly_file_dest(&tree, &source, "*.txt")
            .expect_err("wildcard dest should fail")
            .contains("cannot contain '*'")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn expand_simple_glob_sorts_matches_and_allows_missing_directories() {
    let root = unique_dir("gaia-assembly-glob");
    let spec = test_spec(&root);
    let glob_dir = root.join("glob");
    fs::create_dir_all(&glob_dir).expect("glob dir");
    fs::write(glob_dir.join("b.dtb"), "b").expect("b");
    fs::write(glob_dir.join("a.dtb"), "a").expect("a");
    fs::write(glob_dir.join("skip.txt"), "skip").expect("skip");
    let roots = AssemblyRoots {
        assembly_work: root.join("build/assembly"),
        assembly_out: root.join("out/images"),
        provider_images: root.join("out/images"),
        provider_target: root.join("out/images/buildroot-output/target"),
        provider_host: None,
        provider_staging: None,
        trees: std::collections::BTreeMap::new(),
    };

    let matches = expand_simple_glob(&spec, &roots, &glob_dir.join("*.dtb").display().to_string())
        .expect("glob matches");
    assert_eq!(
        matches,
        vec![glob_dir.join("a.dtb"), glob_dir.join("b.dtb")]
    );

    let missing = expand_simple_glob(
        &spec,
        &roots,
        &root.join("missing/*.dtb").display().to_string(),
    )
    .expect("missing glob dir");
    assert!(missing.is_empty());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn expand_simple_glob_matches_versioned_parent_directories() {
    let root = unique_dir("gaia-assembly-nested-glob");
    let spec = test_spec(&root);
    let firmware = root.join("build/rpi-firmware-1.2.3/boot/overlays");
    fs::create_dir_all(&firmware).expect("firmware dir");
    fs::write(firmware.join("hat_map.dtb"), "hat").expect("hat map");
    fs::write(firmware.join("skip.txt"), "skip").expect("skip");
    let roots = AssemblyRoots {
        assembly_work: root.join("build/assembly"),
        assembly_out: root.join("out/images"),
        provider_images: root.join("out/images"),
        provider_target: root.join("out/images/buildroot-output/target"),
        provider_host: None,
        provider_staging: None,
        trees: std::collections::BTreeMap::new(),
    };

    let pattern = root
        .join("build/rpi-firmware-*/boot/overlays/*.dtb")
        .display()
        .to_string();
    let matches = expand_simple_glob(&spec, &roots, &pattern).expect("nested glob matches");
    assert_eq!(matches, vec![firmware.join("hat_map.dtb")]);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn file_sha256_hashes_sparse_files_incrementally() {
    use std::io::{Seek, SeekFrom, Write};

    let root = unique_dir("gaia-assembly-sha256");
    let path = root.join("sparse.bin");
    let mut file = fs::File::create(&path).expect("sparse file");
    file.write_all(b"prefix").expect("prefix");
    file.seek(SeekFrom::Start(2 * 1024 * 1024)).expect("seek");
    file.write_all(b"suffix").expect("suffix");
    drop(file);

    let mut expected = Sha256::new();
    expected.update(b"prefix");
    let mut position = "prefix".len() as u64;
    let zero_chunk = [0u8; 8192];
    while position < 2 * 1024 * 1024 {
        let remaining = (2 * 1024 * 1024 - position) as usize;
        let take = remaining.min(zero_chunk.len());
        expected.update(&zero_chunk[..take]);
        position += take as u64;
    }
    expected.update(b"suffix");
    let expected = expected
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();

    assert_eq!(file_sha256(&path).expect("sha256"), expected);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn read_tail_bytes_keeps_only_configured_tail() {
    let input = b"0123456789abcdef".as_slice();

    assert_eq!(
        read_tail_bytes(input, 6).expect("tail bytes"),
        b"abcdef".to_vec()
    );
    assert!(
        read_tail_bytes(b"discarded".as_slice(), 0)
            .expect("zero retained")
            .is_empty()
    );
}

#[test]
fn publish_assembly_output_restores_previous_output_when_replacement_fails() {
    let root = unique_dir("gaia-assembly-publish-restore");
    let output = root.join("output.img");
    let missing_temp = root.join(".output.img.gaia-tmp");
    fs::create_dir_all(&root).expect("root dir");
    fs::write(&output, "previous").expect("previous output");

    let error = publish_assembly_output(&missing_temp, &output).expect_err("publish failure");

    assert!(
        error.contains("previous assembly output was restored"),
        "{error}"
    );
    assert_eq!(
        fs::read_to_string(&output).expect("restored output"),
        "previous"
    );
    assert!(!temporary_assembly_backup_path(&output).exists());
}

#[test]
fn stage_image_assembly_cleans_tree_and_skips_optional_missing_sources() {
    let root = unique_dir("gaia-assembly-stage");
    let mut spec = test_spec(&root);
    let source = root.join("assets/config.txt");
    fs::create_dir_all(source.parent().expect("source parent")).expect("assets");
    fs::write(&source, "config").expect("source");
    let tree = root.join("build/assembly/boot");
    fs::create_dir_all(&tree).expect("tree");
    fs::write(tree.join("stale"), "stale").expect("stale");

    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some(root.join("build/assembly").display().to_string().into()),
        trees: vec![AssemblyTreeSpec {
            id: "boot".into(),
            path: "$assembly.work/boot".into(),
        }],
        files: vec![
            AssemblyFileSpec {
                tree: "boot".into(),
                src: Some("@assets/config.txt".into()),
                src_glob: None,
                dest: "config.txt".into(),
                mode: None,
                optional: false,
                preserve_symlink: false,
            },
            AssemblyFileSpec {
                tree: "boot".into(),
                src: Some("@assets/missing.txt".into()),
                src_glob: None,
                dest: "missing.txt".into(),
                mode: None,
                optional: true,
                preserve_symlink: false,
            },
        ],
        ..ImageAssemblySpec::default()
    });

    let summary = stage_image_assembly(&spec, &OperationId::image_assembly(), None)
        .expect("assembly staging");
    assert_eq!(
        fs::read_to_string(tree.join("config.txt")).unwrap(),
        "config"
    );
    assert!(!tree.join("stale").exists());
    assert!(!tree.join("missing.txt").exists());
    assert_eq!(summary.cleanup_paths, vec![tree]);
    let state = summary.state.render();
    assert!(state.contains("staged_file_count=1"));
    assert!(state.contains("skipped_file_count=1"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn copy_assembly_file_copies_regular_files_and_applies_mode() {
    let root = unique_dir("gaia-assembly-copy");
    let source = root.join("source");
    let dest = root.join("nested/dest");
    fs::write(&source, "payload").expect("source");
    let file = AssemblyFileSpec {
        tree: "boot".into(),
        src: Some(source.display().to_string().into()),
        src_glob: None,
        dest: "dest".into(),
        mode: Some("0755".into()),
        optional: false,
        preserve_symlink: false,
    };

    copy_assembly_file(&source, &dest, &file).expect("copy");
    assert_eq!(fs::read_to_string(&dest).unwrap(), "payload");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&dest).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn copy_assembly_file_preserves_symlinks_when_requested() {
    let root = unique_dir("gaia-assembly-symlink");
    let source_target = root.join("target");
    let source_link = root.join("link");
    let dest = root.join("dest-link");
    fs::write(&source_target, "payload").expect("target");
    std::os::unix::fs::symlink("target", &source_link).expect("symlink");
    let file = AssemblyFileSpec {
        tree: "boot".into(),
        src: Some(source_link.display().to_string().into()),
        src_glob: None,
        dest: "dest-link".into(),
        mode: None,
        optional: false,
        preserve_symlink: true,
    };

    copy_assembly_file(&source_link, &dest, &file).expect("copy symlink");
    assert_eq!(
        fs::read_link(&dest).expect("dest symlink"),
        Path::new("target")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn assembly_disk_helpers_parse_types_aliases_and_signatures() {
    let raw = gaia_spec::AssemblyDiskPartitionSpec {
        name: "raw".into(),
        kind: Some("0x0C".into()),
        type_alias: None,
        bootable: false,
        image: "boot.vfat".into(),
    };
    let alias = gaia_spec::AssemblyDiskPartitionSpec {
        name: "alias".into(),
        kind: None,
        type_alias: Some("fat32-lba".into()),
        bootable: false,
        image: "boot.vfat".into(),
    };
    let linux = gaia_spec::AssemblyDiskPartitionSpec {
        name: "linux".into(),
        kind: None,
        type_alias: Some("linux".into()),
        bootable: false,
        image: "rootfs".into(),
    };
    let disk = gaia_spec::AssemblyDiskSpec {
        id: "sdcard".into(),
        output: "sdcard.img".into(),
        partition_table: gaia_spec::AssemblyPartitionTableSpec::Mbr,
        signature: Some("0x48454c49".into()),
        signature_text: None,
        first_lba: None,
        alignment_lba: None,
        partitions: Vec::new(),
    };

    assert_eq!(partition_type_byte(&raw).expect("raw type"), 0x0c);
    assert_eq!(partition_type_byte(&alias).expect("alias type"), 0x0c);
    assert_eq!(partition_type_byte(&linux).expect("linux type"), 0x83);
    assert_eq!(
        disk_signature_bytes(&disk).expect("signature"),
        Some([0x49, 0x4c, 0x45, 0x48])
    );
    assert_eq!(align_to(2049, 2048), 4096);
}

#[cfg(unix)]
#[test]
fn busybox_applet_symlink_points_to_busybox() {
    let root = unique_dir("gaia-busybox-applet");
    let tree = root.join("initramfs");
    fs::create_dir_all(tree.join("bin")).expect("bin dir");
    fs::write(tree.join("bin/busybox"), "busybox").expect("busybox");

    create_busybox_applet_symlink(&tree, "sh").expect("applet symlink");

    assert_eq!(
        fs::read_link(tree.join("bin/sh")).expect("sh link"),
        Path::new("busybox")
    );
    assert!(create_busybox_applet_symlink(&tree, "../bad").is_err());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn ldd_parser_extracts_dynamic_library_paths_and_skips_vdso() {
    assert_eq!(
        parse_ldd_library_path("libc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0xabc)")
            .expect("libc"),
        PathBuf::from("/lib/x86_64-linux-gnu/libc.so.6")
    );
    assert_eq!(
        parse_ldd_library_path("/lib64/ld-linux-x86-64.so.2 (0xabc)").expect("loader"),
        PathBuf::from("/lib64/ld-linux-x86-64.so.2")
    );
    assert!(parse_ldd_library_path("linux-vdso.so.1 (0xabc)").is_none());
    assert!(parse_ldd_library_path("libmissing.so => not found").is_none());
}

#[test]
fn busybox_runtime_parser_distinguishes_static_dynamic_and_failed_resolution() {
    let busybox = Path::new("/tmp/busybox");

    assert!(
        parse_busybox_runtime_libraries_from_ldd("not a dynamic executable", false, busybox)
            .expect("static busybox")
            .is_empty()
    );
    assert_eq!(
        parse_busybox_runtime_libraries_from_ldd(
            "libc.so.6 => /lib/libc.so.6 (0x1)\n/lib64/ld-linux.so.2 (0x2)\n",
            true,
            busybox,
        )
        .expect("dynamic busybox"),
        vec![
            PathBuf::from("/lib/libc.so.6"),
            PathBuf::from("/lib64/ld-linux.so.2")
        ]
    );
    assert!(
        parse_busybox_runtime_libraries_from_ldd("libmissing.so => not found", false, busybox)
            .expect_err("failed ldd")
            .contains("failed to resolve busybox runtime libraries")
    );
}

#[cfg(unix)]
#[test]
fn busybox_runtime_resolver_honors_assembly_command_timeout() {
    use std::os::unix::fs::PermissionsExt;

    let root = unique_dir("gaia-busybox-ldd-timeout");
    let mut spec = test_spec(&root);
    spec.policy.providers.buildroot.timeout_seconds = 1;
    let busybox = root.join("busybox");
    fs::write(&busybox, "busybox").expect("busybox");
    let ldd = root.join("ldd");
    fs::write(&ldd, "#!/bin/sh\nsleep 10\n").expect("fake ldd");
    let mut permissions = fs::metadata(&ldd).expect("fake ldd metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&ldd, permissions).expect("fake ldd executable");

    let started = Instant::now();
    let error = resolve_busybox_runtime_libraries_with_program(&spec, &busybox, &ldd, None)
        .expect_err("ldd should time out");

    assert!(started.elapsed().as_secs() < 5, "{error:?}");
    assert_eq!(error.kind, ExecutionErrorKind::Timeout);
    assert!(error.message.contains("timed out after 1s"), "{error:?}");

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn busybox_runtime_resolver_honors_cancellation() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::thread;
    use std::time::Duration;

    let root = unique_dir("gaia-busybox-ldd-cancel");
    let mut spec = test_spec(&root);
    spec.policy.providers.buildroot.timeout_seconds = 30;
    let busybox = root.join("busybox");
    fs::write(&busybox, "busybox").expect("busybox");
    let ldd = root.join("ldd");
    fs::write(&ldd, "#!/bin/sh\nsleep 10\n").expect("fake ldd");
    let mut permissions = fs::metadata(&ldd).expect("fake ldd metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&ldd, permissions).expect("fake ldd executable");
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancel_check: gaia_process::ProcessCancelCheck = {
        let cancelled = cancelled.clone();
        Arc::new(move || cancelled.load(Ordering::SeqCst))
    };
    let trigger = cancelled.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        trigger.store(true, Ordering::SeqCst);
    });

    let started = Instant::now();
    let error =
        resolve_busybox_runtime_libraries_with_program(&spec, &busybox, &ldd, Some(cancel_check))
            .expect_err("ldd should be cancelled");

    assert!(started.elapsed().as_secs() < 5, "{error:?}");
    assert_eq!(error.kind, ExecutionErrorKind::Cancelled);
    assert!(error.message.contains("cancelled"), "{error:?}");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn buildroot_assembly_roots_expose_images_target_host_and_staging() {
    let root = unique_dir("gaia-buildroot-assembly-roots");
    let mut spec = test_spec(&root);
    spec.image.output = ImageOutputSpec {
        collect_dir: Some(root.join("publish").display().to_string()),
        archive_name: None,
        emit_report: true,
    };
    let assembly = ImageAssemblySpec {
        work_dir: Some("$provider.images/work".into()),
        out_dir: Some("$provider.images".into()),
        trees: vec![AssemblyTreeSpec {
            id: "roots".into(),
            path: "$assembly.work/tree".into(),
        }],
        ..ImageAssemblySpec::default()
    };

    let roots = AssemblyRoots::new(&spec, &assembly).expect("assembly roots");

    assert_eq!(roots.provider_images, root.join("publish"));
    assert_eq!(
        roots.provider_target,
        root.join("publish/buildroot-output/target")
    );
    assert_eq!(
        roots.provider_host.as_deref(),
        Some(root.join("publish/buildroot-output/host").as_path())
    );
    assert_eq!(
        roots.provider_staging.as_deref(),
        Some(root.join("publish/buildroot-output/staging").as_path())
    );
    assert_eq!(
        roots
            .resolve_path(&spec, "$provider.host/bin/dtc")
            .expect("host"),
        root.join("publish/buildroot-output/host/bin/dtc")
    );
    assert_eq!(
        roots.tree_path("roots").expect("tree"),
        root.join("publish/work/tree")
    );

    let _ = fs::remove_dir_all(root);
}
