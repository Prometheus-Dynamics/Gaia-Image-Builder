use super::*;
use gaia_spec::expand_simple_glob;
use gaia_testkit::TempDir;

use gaia_spec::{
    AssemblyFileSpec, AssemblyTreeSpec, ImageAssemblySpec, ImageOutputSpec, WorkspaceNamedPathSpec,
    WorkspacePathKindSpec,
};
use std::fs;
use std::path::Path;

pub(super) fn test_spec(root: &Path) -> ResolvedBuildSpec {
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
        disk: None,
        collect_dir: Some(root.join("out/images").display().to_string()),
        archive_name: None,
        emit_report: true,
    };
    spec
}

#[test]
fn assembly_file_dest_maps_directory_dest_and_rejects_escape() {
    let scratch = TempDir::new("gaia-exec-test");
    let root = scratch.join("gaia-assembly-dest");
    fs::create_dir_all(&root).expect("test root");
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
}

#[test]
fn expand_simple_glob_sorts_matches_and_allows_missing_directories() {
    let scratch = TempDir::new("gaia-exec-test");
    let root = scratch.join("gaia-assembly-glob");
    fs::create_dir_all(&root).expect("test root");
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
}

#[test]
fn expand_simple_glob_matches_versioned_parent_directories() {
    let scratch = TempDir::new("gaia-exec-test");
    let root = scratch.join("gaia-assembly-nested-glob");
    fs::create_dir_all(&root).expect("test root");
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
}

#[test]
fn file_sha256_hashes_sparse_files_incrementally() {
    let scratch = TempDir::new("gaia-exec-test");
    use std::io::{Seek, SeekFrom, Write};

    let root = scratch.join("gaia-assembly-sha256");
    fs::create_dir_all(&root).expect("test root");
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
    let scratch = TempDir::new("gaia-exec-test");
    let root = scratch.join("gaia-assembly-publish-restore");
    fs::create_dir_all(&root).expect("test root");
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
    let scratch = TempDir::new("gaia-exec-test");
    let root = scratch.join("gaia-assembly-stage");
    fs::create_dir_all(&root).expect("test root");
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
            strip: false,
            id: "boot".into(),
            path: "$assembly.work/boot".into(),
        }],
        files: vec![
            AssemblyFileSpec {
                include: Vec::new(),
                strip: false,
                exclude: Vec::new(),
                tree: "boot".into(),
                src: Some("@assets/config.txt".into()),
                src_glob: None,
                dest: "config.txt".into(),
                mode: None,
                optional: false,
                preserve_symlink: false,
                allow_missing_libs: false,
                content: None,
                json: None,
            },
            AssemblyFileSpec {
                include: Vec::new(),
                strip: false,
                exclude: Vec::new(),
                tree: "boot".into(),
                src: Some("@assets/missing.txt".into()),
                src_glob: None,
                dest: "missing.txt".into(),
                mode: None,
                optional: true,
                preserve_symlink: false,
                allow_missing_libs: false,
                content: None,
                json: None,
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
}

#[test]
fn copy_assembly_file_copies_regular_files_and_applies_mode() {
    let scratch = TempDir::new("gaia-exec-test");
    let root = scratch.join("gaia-assembly-copy");
    fs::create_dir_all(&root).expect("test root");
    let source = root.join("source");
    let dest = root.join("nested/dest");
    fs::write(&source, "payload").expect("source");
    let file = AssemblyFileSpec {
        include: Vec::new(),
        strip: false,
        exclude: Vec::new(),
        tree: "boot".into(),
        src: Some(source.display().to_string().into()),
        src_glob: None,
        dest: "dest".into(),
        mode: Some("0755".into()),
        optional: false,
        preserve_symlink: false,
        allow_missing_libs: false,
        content: None,
        json: None,
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
}

#[cfg(unix)]
#[test]
fn copy_assembly_file_preserves_symlinks_when_requested() {
    let scratch = TempDir::new("gaia-exec-test");
    let root = scratch.join("gaia-assembly-symlink");
    fs::create_dir_all(&root).expect("test root");
    let source_target = root.join("target");
    let source_link = root.join("link");
    let dest = root.join("dest-link");
    fs::write(&source_target, "payload").expect("target");
    std::os::unix::fs::symlink("target", &source_link).expect("symlink");
    let file = AssemblyFileSpec {
        include: Vec::new(),
        strip: false,
        exclude: Vec::new(),
        tree: "boot".into(),
        src: Some(source_link.display().to_string().into()),
        src_glob: None,
        dest: "dest-link".into(),
        mode: None,
        optional: false,
        preserve_symlink: true,
        allow_missing_libs: false,
        content: None,
        json: None,
    };

    copy_assembly_file(&source_link, &dest, &file).expect("copy symlink");
    assert_eq!(
        fs::read_link(&dest).expect("dest symlink"),
        Path::new("target")
    );
}

#[test]
fn assembly_disk_helpers_parse_types_aliases_and_signatures() {
    let raw = gaia_spec::AssemblyDiskPartitionSpec {
        name: "raw".into(),
        kind: Some("0x0C".into()),
        type_alias: None,
        bootable: false,
        image: Some("boot.vfat".into()),
        size: None,
        wipe: false,
        materialize: true,
        gpt: Default::default(),
    };
    let alias = gaia_spec::AssemblyDiskPartitionSpec {
        name: "alias".into(),
        kind: None,
        type_alias: Some("fat32-lba".into()),
        bootable: false,
        image: Some("boot.vfat".into()),
        size: None,
        wipe: false,
        materialize: true,
        gpt: Default::default(),
    };
    let linux = gaia_spec::AssemblyDiskPartitionSpec {
        name: "linux".into(),
        kind: None,
        type_alias: Some("linux".into()),
        bootable: false,
        image: Some("rootfs".into()),
        size: None,
        wipe: false,
        materialize: true,
        gpt: Default::default(),
    };
    let disk = gaia_spec::AssemblyDiskSpec {
        id: "sdcard".into(),
        output: "sdcard.img".into(),
        partition_table: gaia_spec::AssemblyPartitionTableSpec::Mbr,
        signature: Some("0x48454c49".into()),
        signature_text: None,
        first_lba: None,
        alignment_lba: None,
        truncate: None,
        ebr_placement: gaia_spec::AssemblyEbrPlacementSpec::Default,
        partitions: Vec::new(),
        gpt: Default::default(),
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
    let scratch = TempDir::new("gaia-exec-test");
    let root = scratch.join("gaia-busybox-applet");
    fs::create_dir_all(&root).expect("test root");
    let tree = root.join("initramfs");
    fs::create_dir_all(tree.join("bin")).expect("bin dir");
    fs::write(tree.join("bin/busybox"), "busybox").expect("busybox");

    create_busybox_applet_symlink(&tree, "sh").expect("applet symlink");

    assert_eq!(
        fs::read_link(tree.join("bin/sh")).expect("sh link"),
        Path::new("busybox")
    );
    assert!(create_busybox_applet_symlink(&tree, "../bad").is_err());
}
#[test]
fn buildroot_assembly_roots_expose_images_target_host_and_staging() {
    let scratch = TempDir::new("gaia-exec-test");
    let root = scratch.join("gaia-buildroot-assembly-roots");
    fs::create_dir_all(&root).expect("test root");
    let mut spec = test_spec(&root);
    spec.image.output = ImageOutputSpec {
        disk: None,
        collect_dir: Some(root.join("publish").display().to_string()),
        archive_name: None,
        emit_report: true,
    };
    let assembly = ImageAssemblySpec {
        work_dir: Some("$provider.images/work".into()),
        out_dir: Some("$provider.images".into()),
        trees: vec![AssemblyTreeSpec {
            strip: false,
            id: "roots".into(),
            path: "$assembly.work/tree".into(),
        }],
        ..ImageAssemblySpec::default()
    };

    let roots = AssemblyRoots::new(&spec, &assembly).expect("assembly roots");

    assert_eq!(roots.provider_images, root.join("publish"));
    assert_eq!(
        roots.provider_target,
        root.join("build/image/buildroot-output/target")
    );
    assert_eq!(
        roots.provider_host.as_deref(),
        Some(root.join("build/image/buildroot-output/host").as_path())
    );
    assert_eq!(
        roots.provider_staging.as_deref(),
        Some(root.join("build/image/buildroot-output/staging").as_path())
    );
    assert_eq!(
        roots
            .resolve_path(&spec, "$provider.host/bin/dtc")
            .expect("host"),
        root.join("build/image/buildroot-output/host/bin/dtc")
    );
    assert_eq!(
        roots.tree_path("roots").expect("tree"),
        root.join("publish/work/tree")
    );
}

fn boot_assembly(
    root: &Path,
    transforms: Vec<gaia_spec::AssemblyTransformSpec>,
) -> ImageAssemblySpec {
    ImageAssemblySpec {
        work_dir: Some(root.join("build/assembly").display().to_string().into()),
        trees: vec![AssemblyTreeSpec {
            strip: false,
            id: "boot".into(),
            path: "$assembly.work/boot".into(),
        }],
        files: vec![AssemblyFileSpec {
            include: Vec::new(),
            strip: false,
            exclude: Vec::new(),
            tree: "boot".into(),
            src: Some("@assets/config.txt".into()),
            src_glob: None,
            dest: "config.txt".into(),
            mode: None,
            optional: false,
            preserve_symlink: false,
            allow_missing_libs: false,
            content: None,
            json: None,
        }],
        transforms,
        filesystems: vec![gaia_spec::AssemblyFilesystemSpec {
            free: None,
            id: "boot".into(),
            kind: gaia_spec::AssemblyFilesystemKindSpec::Cpio,
            source_tree: "boot".into(),
            output: "$assembly.work/boot.cpio".into(),
            size: None,
            deterministic: true,
            compression_level: None,
            publish: false,
            label: None,
            uuid: None,
            journal: None,
            compressed: Default::default(),
            fat_size: None,
            reserved_percent: None,
        }],
        ..ImageAssemblySpec::default()
    }
}

fn copy_transform(src: &str, dest: &str) -> gaia_spec::AssemblyTransformSpec {
    gaia_spec::AssemblyTransformSpec {
        kind: gaia_spec::AssemblyTransformKindSpec::Copy,
        src: Some(src.into()),
        dest: dest.into(),
        deterministic: true,
        level: None,
        hash_dest: None,
        data_block_size: None,
        hash_block_size: None,
        long: false,
        salt: None,
        uuid: None,
    }
}

#[test]
fn transforms_read_this_runs_filesystem_output() {
    let scratch = TempDir::new("gaia-exec-test");
    let root = scratch.join("gaia-assembly-order");
    fs::create_dir_all(&root).expect("test root");
    let mut spec = test_spec(&root);
    let source = root.join("assets/config.txt");
    fs::create_dir_all(source.parent().expect("source parent")).expect("assets");
    fs::write(&source, "kernel 7.2.9").expect("source");
    // The previous run's image, which the transform must not pick up.
    let work = root.join("build/assembly");
    fs::create_dir_all(&work).expect("work");
    fs::write(work.join("boot.cpio"), "kernel 6.12.47").expect("stale image");

    // Declared (and of a kind classically run) before the filesystem it reads.
    spec.image.assembly = Some(boot_assembly(
        &root,
        vec![copy_transform(
            "$assembly.work/boot.cpio",
            "$assembly.work/boot.cpio.update",
        )],
    ));
    let summary = stage_image_assembly(&spec, &OperationId::image_assembly(), None)
        .expect("assembly staging");

    let image = fs::read(work.join("boot.cpio")).expect("image");
    let update = fs::read(work.join("boot.cpio.update")).expect("update");
    assert_eq!(update, image);
    assert!(String::from_utf8_lossy(&update).contains("kernel 7.2.9"));
    let position = |text: &str| {
        summary
            .messages
            .iter()
            .position(|message| message.contains(text))
            .unwrap_or_else(|| panic!("no message with '{text}': {:?}", summary.messages))
    };
    assert!(position("built assembly filesystem 'boot'") < position("ran assembly transform"));
}

#[test]
fn assembly_steps_reading_each_others_outputs_are_refused() {
    let scratch = TempDir::new("gaia-exec-test");
    let root = scratch.join("gaia-assembly-cycle");
    fs::create_dir_all(&root).expect("test root");
    let mut spec = test_spec(&root);
    let source = root.join("assets/config.txt");
    fs::create_dir_all(source.parent().expect("source parent")).expect("assets");
    fs::write(&source, "config").expect("source");
    spec.image.assembly = Some(boot_assembly(
        &root,
        vec![
            copy_transform("$assembly.work/a", "$assembly.work/b"),
            copy_transform("$assembly.work/b", "$assembly.work/a"),
        ],
    ));
    let error =
        stage_image_assembly(&spec, &OperationId::image_assembly(), None).expect_err("cycle");
    assert!(error.message.contains("in a cycle"), "{}", error.message);
    assert!(
        error
            .message
            .contains("copy transform to '$assembly.work/b'")
    );
}

#[test]
fn assembly_disk_archives_log_a_step_time_and_sizes() {
    let scratch = TempDir::new("gaia-exec-test");
    for (archive_name, tool, label) in [
        ("acme.img.xz", "xz", "xz"),
        ("acme.img.zst", "zstd", "zstd"),
    ] {
        if Command::new(tool).arg("--version").output().is_err() {
            eprintln!("skipping assembly archive step-time test: '{tool}' is not installed");
            continue;
        }
        let root = scratch.join("gaia-assembly-archive-step-time");
        fs::create_dir_all(&root).expect("test root");
        let mut spec = test_spec(&root);
        spec.image.output.archive_name = Some(archive_name.into());
        let images = root.join("out/images");
        fs::create_dir_all(&images).expect("images dir");
        let source = images.join("sdcard.img");
        fs::write(&source, "raw disk ".repeat(4096)).expect("disk");

        let mut messages = Vec::new();
        let built = [("sdcard".to_string(), source.clone())];
        let summary = archive_assembly_disk_output(&spec, &built, None, &mut messages)
            .expect("archive should succeed")
            .expect("archive summary");
        assert_eq!(summary.output, images.join(archive_name));

        let step = format!("archive {archive_name} ({label})");
        assert!(
            messages.iter().any(|message| {
                gaia_process::parse_step_time(message).is_some_and(|(name, _)| name == step)
            }),
            "missing step time '{step}' in {messages:?}"
        );
        assert!(
            messages.iter().any(|message| message
                .starts_with(&format!("archived {archive_name}: "))
                && message.contains(" -> ")),
            "missing size line in {messages:?}"
        );
    }
}

#[path = "tests_binaries.rs"]
mod tests_binaries;
#[path = "tests_busybox.rs"]
mod tests_busybox;
#[path = "tests_ext4.rs"]
mod tests_ext4;
#[path = "tests_file_libs.rs"]
mod tests_file_libs;
#[path = "tests_filesystem.rs"]
mod tests_filesystem;
#[path = "tests_generated.rs"]
mod tests_generated;
#[path = "tests_gpt.rs"]
mod tests_gpt;
#[path = "tests_schedule.rs"]
mod tests_schedule;

#[path = "tests_compressed_fs.rs"]
mod tests_compressed_fs;
#[path = "tests_cpio_xz.rs"]
mod tests_cpio_xz;
#[path = "tests_entries.rs"]
mod tests_entries;
#[path = "tests_kernel_modules.rs"]
mod tests_kernel_modules;
#[path = "tests_strip.rs"]
mod tests_strip;
#[path = "tests_verity.rs"]
mod tests_verity;

#[path = "tests_ram.rs"]
mod ram;

/// Buildroot installs system tools (veritysetup, mkfs.ext4, debugfs) into
/// host/sbin: the provider's host tools are searched there too, before PATH.
#[test]
fn assembly_tools_are_found_in_the_provider_host_sbin() {
    let scratch = gaia_testkit::TempDir::new("gaia-assembly-host-sbin");
    let root = scratch.path().to_path_buf();
    let host = root.join("host");
    fs::create_dir_all(host.join("sbin")).expect("host sbin");
    let tool = host.join("sbin/gaia-test-only-veritysetup");
    fs::write(&tool, "#!/bin/sh\n").expect("tool");
    let roots = AssemblyRoots {
        assembly_work: root.join("build/assembly"),
        assembly_out: root.join("out/images"),
        provider_images: root.join("out/images"),
        provider_target: root.join("target"),
        provider_host: Some(host.clone()),
        provider_staging: None,
        trees: std::collections::BTreeMap::new(),
    };
    let resolved =
        resolve_assembly_tool(&roots, "gaia-test-only-veritysetup").expect("found in host/sbin");
    assert_eq!(resolved.program, tool);
}
