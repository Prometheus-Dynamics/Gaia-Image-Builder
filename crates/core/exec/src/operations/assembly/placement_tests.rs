//! Tests for the placement decision, the intermediate moves and the
//! sparse copy. Split from `placement.rs` for its length.

use super::*;

fn roots_for(root: &Path) -> (ResolvedBuildSpec, AssemblyRoots) {
    let mut spec = ResolvedBuildSpec::new("placement-test");
    spec.workspace.root_dir = root.display().to_string();
    spec.workspace.build_dir = root.join("build").display().to_string();
    spec.workspace.out_dir = root.join("out").display().to_string();
    spec.image.output.collect_dir = Some(root.join("out/images").display().to_string());
    spec.image.assembly = Some(ImageAssemblySpec {
        work_dir: Some("ram".into()),
        trees: vec![gaia_spec::AssemblyTreeSpec {
            id: "boot".into(),
            path: "$assembly.work/boot".into(),
        }],
        filesystems: vec![gaia_spec::AssemblyFilesystemSpec {
            id: "bootfs".into(),
            kind: gaia_spec::AssemblyFilesystemKindSpec::Vfat,
            source_tree: "boot".into(),
            output: "$provider.images/boot.vfat".into(),
            size: Some("4M".into()),
            deterministic: true,
        }],
        ..ImageAssemblySpec::default()
    });
    let assembly = spec.image.assembly.clone().expect("assembly");
    let roots = AssemblyRoots::new(&spec, &assembly).expect("roots");
    (spec, roots)
}

fn env(base: &Path, memory: Option<u64>, tmpfs: Option<u64>) -> PlacementEnv {
    PlacementEnv {
        ram_base: base.to_path_buf(),
        user: "tester".into(),
        memory_available: memory,
        tmpfs_available: tmpfs,
    }
}

#[test]
fn ram_moves_intermediates_and_keeps_published_disks_on_disk() {
    let root = std::env::temp_dir().join(format!("gaia-placement-moves-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (mut spec, roots) = roots_for(&root);
    let mut assembly = spec.image.assembly.clone().expect("assembly");
    assembly.disks.push(gaia_spec::AssemblyDiskSpec {
        id: "sdcard".into(),
        output: "$provider.images/sdcard.img".into(),
        partition_table: gaia_spec::AssemblyPartitionTableSpec::Mbr,
        signature: None,
        signature_text: None,
        first_lba: None,
        alignment_lba: None,
        partitions: vec![gaia_spec::AssemblyDiskPartitionSpec {
            name: "boot".into(),
            kind: None,
            type_alias: Some("fat32-lba".into()),
            bootable: true,
            image: Some("$provider.images/boot.vfat".into()),
            size: None,
            wipe: false,
        }],
    });
    spec.image.assembly = Some(assembly.clone());
    let base = root.join("ram");
    let placement = decide_placement(&spec, &assembly, &roots, || {
        env(&base, Some(64 * GIB), Some(64 * GIB))
    });
    let AssemblyPlacement::Ram(ram) = placement else {
        panic!("expected a RAM placement, got {placement:?}");
    };

    // Trees, the work dir and filesystem outputs are in RAM.
    assert!(ram.root.starts_with(&base));
    let tree = ram.roots.tree_path("boot").expect("tree").to_path_buf();
    assert_eq!(tree, ram.root.join("trees/boot"));
    let boot_vfat = ram
        .roots
        .resolve_path(&spec, &ram.assembly.filesystems[0].output)
        .expect("vfat");
    assert_eq!(boot_vfat, ram.root.join("out/boot.vfat"));
    // The disk is built in RAM and published on disk.
    let sdcard = ram
        .roots
        .resolve_path(&spec, &ram.assembly.disks[0].output)
        .expect("sdcard");
    assert_eq!(sdcard, ram.root.join("out/sdcard.img"));
    assert_eq!(
        ram.roots
            .resolve_path(
                &spec,
                ram.assembly.disks[0].partitions[0]
                    .image
                    .clone()
                    .expect("image"),
            )
            .expect("partition image"),
        boot_vfat
    );
    assert_eq!(
        ram.disk_publish,
        vec![
            (roots
                .resolve_path(&spec, "$provider.images/sdcard.img")
                .expect("disk"))
        ]
    );
    // Every disk is published to its spec path, under the work dir too.
    assert!(
        ram.stale
            .contains(&roots.tree_path("boot").expect("tree").to_path_buf())
    );
    // Templates are absolute now, so the spec's own view is unchanged.
    assert_eq!(
        roots
            .resolve_path(&spec, "$provider.images/sdcard.img")
            .expect("disk view"),
        PathBuf::from(&spec.workspace.out_dir).join("images/sdcard.img")
    );
    assert!(ram.messages[0].contains("in RAM"));
}

#[test]
fn ram_falls_back_to_disk_when_the_intermediates_do_not_fit() {
    let root = std::env::temp_dir().join(format!("gaia-placement-fit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (spec, roots) = roots_for(&root);
    let assembly = spec.image.assembly.clone().expect("assembly");
    let base = root.join("ram");

    let short_memory = decide_placement(&spec, &assembly, &roots, || {
        env(&base, Some(GIB), Some(64 * GIB))
    });
    let AssemblyPlacement::Disk { messages } = short_memory else {
        panic!("expected a disk fallback");
    };
    assert!(messages[0].contains("building on disk"), "{messages:?}");
    assert!(messages[0].contains("of memory available"), "{messages:?}");

    let unknown_tmpfs = decide_placement(&spec, &assembly, &roots, || {
        env(&base, Some(64 * GIB), None)
    });
    assert!(matches!(unknown_tmpfs, AssemblyPlacement::Disk { .. }));
}

#[test]
fn disk_keyword_and_paths_never_go_to_ram() {
    let root = std::env::temp_dir().join(format!("gaia-placement-disk-{}", std::process::id()));
    let (mut spec, roots) = roots_for(&root);
    let mut assembly = spec.image.assembly.clone().expect("assembly");
    assembly.work_dir = Some("disk".into());
    spec.image.assembly = Some(assembly.clone());
    let never = || -> PlacementEnv { panic!("the system must not be read for disk") };
    assert!(matches!(
        decide_placement(&spec, &assembly, &roots, never),
        AssemblyPlacement::Disk { ref messages } if messages.is_empty()
    ));
    assembly.work_dir = Some("$provider.images/work".into());
    assert!(matches!(
        decide_placement(&spec, &assembly, &roots, never),
        AssemblyPlacement::Disk { .. }
    ));
}

#[test]
fn unset_work_dir_follows_the_buildroot_tree() {
    let root = std::env::temp_dir().join(format!("gaia-placement-follow-{}", std::process::id()));
    let (mut spec, roots) = roots_for(&root);
    let mut assembly = spec.image.assembly.clone().expect("assembly");
    assembly.work_dir = None;
    spec.image.assembly = Some(assembly.clone());
    spec.policy.providers.buildroot.work_dir.work_dir = "ram".into();
    assert!(ram_requested(&spec, &assembly));
    spec.policy.providers.buildroot.work_dir.work_dir = "disk".into();
    assert!(!ram_requested(&spec, &assembly));
    spec.policy.providers.buildroot.work_dir.work_dir = "ram".into();
    assert!(!ram_requested(
        &spec,
        &ImageAssemblySpec {
            work_dir: Some("/somewhere".into()),
            ..assembly.clone()
        }
    ));
    let _ = roots;
}

#[test]
fn map_path_takes_the_longest_move() {
    let moves = vec![
        (PathBuf::from("/d/work"), PathBuf::from("/r/work")),
        (
            PathBuf::from("/d/work/boot"),
            PathBuf::from("/r/trees/boot"),
        ),
        (PathBuf::from("/d/out.img"), PathBuf::from("/r/out/out.img")),
    ];
    assert_eq!(
        map_path(&moves, Path::new("/d/work/boot/config.txt")),
        Some(PathBuf::from("/r/trees/boot/config.txt"))
    );
    assert_eq!(
        map_path(&moves, Path::new("/d/work/auto.vfat")),
        Some(PathBuf::from("/r/work/auto.vfat"))
    );
    assert_eq!(
        map_path(&moves, Path::new("/d/out.img")),
        Some(PathBuf::from("/r/out/out.img"))
    );
    assert_eq!(map_path(&moves, Path::new("/elsewhere")), None);
}

#[test]
fn digest_tokens_have_their_paths_rewritten() {
    let rewritten = rewrite_digest_tokens(
        "a=${assembly.sha256:$assembly.out/boot.vfat}/b=${assembly.sha256: x.img }",
        |inner| Ok(format!("<{}>", inner.trim())),
    )
    .expect("rewrite");
    assert_eq!(
        rewritten,
        "a=${assembly.sha256:<$assembly.out/boot.vfat>}/b=${assembly.sha256:<x.img>}"
    );
    assert_eq!(
        rewrite_digest_tokens("plain", |_| Ok(String::new())).expect("plain"),
        "plain"
    );
}

#[test]
fn sparse_copy_writes_the_same_bytes() {
    let root = std::env::temp_dir().join(format!("gaia-placement-sparse-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("root");
    let source = root.join("source.img");
    let mut bytes = vec![0u8; 3 * SPARSE_CHUNK + 1234];
    bytes[10] = 7;
    bytes[2 * SPARSE_CHUNK + 5] = 9;
    let last = bytes.len() - 1;
    bytes[last] = 3;
    std::fs::write(&source, &bytes).expect("source");
    let target = root.join("target.img");
    copy_sparse(&source, &target).expect("copy");
    assert_eq!(std::fs::read(&target).expect("target"), bytes);

    // Trailing zeros are kept in the length even though nothing is written.
    let zeros = root.join("zeros.img");
    std::fs::write(&zeros, vec![0u8; SPARSE_CHUNK + 10]).expect("zeros");
    let copied = root.join("zeros-copy.img");
    copy_sparse(&zeros, &copied).expect("zeros copy");
    assert_eq!(
        std::fs::metadata(&copied).expect("meta").len(),
        SPARSE_CHUNK as u64 + 10
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn ram_root_is_stable_per_work_dir() {
    let first = ram_root(
        Path::new("/dev/shm"),
        "alice",
        Path::new("/w/build/assembly"),
    );
    let again = ram_root(
        Path::new("/dev/shm"),
        "alice",
        Path::new("/w/build/assembly"),
    );
    let other = ram_root(
        Path::new("/dev/shm"),
        "alice",
        Path::new("/w/other/assembly"),
    );
    assert_eq!(first, again);
    assert_ne!(first, other);
    assert!(first.starts_with("/dev/shm/gaia-alice"));
    assert!(first.ends_with("assembly"));
}

#[test]
fn a_raw_disk_under_the_work_dir_is_still_published_to_its_path() {
    let root =
        std::env::temp_dir().join(format!("gaia-placement-work-disk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (mut spec, roots) = roots_for(&root);
    let mut assembly = spec.image.assembly.clone().expect("assembly");
    assembly.disks.push(gaia_spec::AssemblyDiskSpec {
        id: "work".into(),
        output: "$assembly.work/work.img".into(),
        partition_table: gaia_spec::AssemblyPartitionTableSpec::Mbr,
        signature: None,
        signature_text: None,
        first_lba: None,
        alignment_lba: None,
        partitions: Vec::new(),
    });
    spec.image.assembly = Some(assembly.clone());
    let base = root.join("ram");
    let AssemblyPlacement::Ram(ram) = decide_placement(&spec, &assembly, &roots, || {
        env(&base, Some(64 * GIB), Some(64 * GIB))
    }) else {
        panic!("expected a RAM placement");
    };
    // Built in RAM (under the work dir's RAM copy), published under the disk
    // view's work dir, as disk mode would have left it.
    let built = ram
        .roots
        .resolve_path(&spec, &ram.assembly.disks[0].output)
        .expect("built");
    assert!(built.starts_with(&ram.root), "{}", built.display());
    assert_eq!(
        ram.disk_publish[0],
        roots
            .resolve_path(&spec, "$assembly.work/work.img")
            .expect("published")
    );
}
