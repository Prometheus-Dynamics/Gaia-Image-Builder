//! RAM work dir tests: the intermediates in RAM, the published outputs on
//! disk, and the fallback. Split from `tests.rs` for its length.

use super::*;
use std::fs;
use std::path::Path;

fn ram_env(base: &Path, memory: u64) -> PlacementEnv {
    PlacementEnv {
        ram_base: base.to_path_buf(),
        user: "tester".into(),
        memory_available: Some(memory),
        tmpfs_available: Some(64 * placement_gib()),
    }
}

fn placement_gib() -> u64 {
    1024 * 1024 * 1024
}

/// Trees, a copied file, a filesystem image and a disk made of them, with
/// the disk published to the collect dir: every kind of output a RAM work
/// dir moves or keeps.
fn ram_test_assembly(work_dir: &str) -> ImageAssemblySpec {
    let file = |src: &str, dest: &str| AssemblyFileSpec {
        tree: "boot".into(),
        src: Some(src.into()),
        src_glob: None,
        dest: dest.into(),
        mode: None,
        optional: false,
        preserve_symlink: false,
    };
    let partition = |name: &str, image: Option<&str>, size: Option<&str>, wipe: bool| {
        gaia_spec::AssemblyDiskPartitionSpec {
            name: name.into(),
            kind: Some("0x83".into()),
            type_alias: None,
            bootable: false,
            image: image.map(Into::into),
            size: size.map(str::to_string),
            wipe,
            materialize: true,
        }
    };
    ImageAssemblySpec {
        work_dir: Some(work_dir.into()),
        trees: vec![AssemblyTreeSpec {
            id: "boot".into(),
            path: "$assembly.work/boot".into(),
        }],
        files: vec![
            file("@assets/config.txt", "config.txt"),
            file("@assets/part.bin", "part.bin"),
        ],
        transforms: vec![copy_transform(
            "$assembly.tree.boot/part.bin",
            "$assembly.work/part-copy.bin",
        )],
        filesystems: vec![gaia_spec::AssemblyFilesystemSpec {
            id: "boot".into(),
            kind: gaia_spec::AssemblyFilesystemKindSpec::Cpio,
            source_tree: "boot".into(),
            output: "$provider.images/boot.cpio".into(),
            size: None,
            deterministic: true,
        }],
        disks: vec![gaia_spec::AssemblyDiskSpec {
            id: "sdcard".into(),
            output: "$provider.images/disk.img".into(),
            partition_table: gaia_spec::AssemblyPartitionTableSpec::Mbr,
            signature: Some("0x48454c49".into()),
            signature_text: None,
            first_lba: None,
            alignment_lba: None,
            truncate: None,
            ebr_placement: gaia_spec::AssemblyEbrPlacementSpec::Default,
            partitions: vec![
                partition("boot", Some("$provider.images/boot.cpio"), None, false),
                partition("copy", Some("$assembly.work/part-copy.bin"), None, false),
                partition("spare", None, Some("1M"), true),
            ],
        }],
        ..ImageAssemblySpec::default()
    }
}

fn write_ram_test_assets(root: &Path) {
    let assets = root.join("assets");
    fs::create_dir_all(&assets).expect("assets");
    fs::write(assets.join("config.txt"), "config\n").expect("config");
    // Mostly zeros with data at both ends, so the sparse copy has holes.
    let mut part = vec![0u8; 3 * 1024 * 1024 + 17];
    part[0] = 0xAB;
    let last = part.len() - 1;
    part[last] = 0xCD;
    fs::write(assets.join("part.bin"), part).expect("part");
}

#[test]
fn ram_work_dir_keeps_intermediates_in_ram_and_publishes_the_same_outputs() {
    let disk_root = unique_dir("gaia-assembly-ram-disk");
    let ram_root = unique_dir("gaia-assembly-ram-ram");
    let ram_base = unique_dir("gaia-assembly-ram-base");
    write_ram_test_assets(&disk_root);
    write_ram_test_assets(&ram_root);
    let mut disk_spec = test_spec(&disk_root);
    disk_spec.image.assembly = Some(ram_test_assembly("disk"));
    let mut ram_spec = test_spec(&ram_root);
    ram_spec.image.assembly = Some(ram_test_assembly("ram"));
    let operation = OperationId::image_assembly();

    let disk_summary = stage_image_assembly_with(&disk_spec, &operation, None, || {
        panic!("a disk work dir reads no RAM facts")
    })
    .expect("disk assembly");
    let ram_summary = stage_image_assembly_with(&ram_spec, &operation, None, || {
        ram_env(&ram_base, 64 * placement_gib())
    })
    .expect("ram assembly");

    let disk_out = disk_root.join("out/images");
    let ram_out = ram_root.join("out/images");
    // The published disk is the same bytes either way.
    assert_eq!(
        fs::read(disk_out.join("disk.img")).expect("disk image"),
        fs::read(ram_out.join("disk.img")).expect("ram published disk image")
    );
    let state_line = |state: &str, key: &str| {
        state
            .lines()
            .find(|line| line.starts_with(key))
            .map(str::to_string)
    };
    let disk_state = disk_summary.state.render();
    let ram_state = ram_summary.state.render();
    assert_eq!(
        state_line(&disk_state, "disk.1.sha256="),
        state_line(&ram_state, "disk.1.sha256=")
    );
    assert_eq!(
        state_line(&disk_state, "transform.1.sha256="),
        state_line(&ram_state, "transform.1.sha256=")
    );
    // The filesystem image is an intermediate: in RAM, not in the collect dir.
    assert!(disk_out.join("boot.cpio").is_file());
    assert!(!ram_out.join("boot.cpio").exists());
    assert!(ram_state.contains("work_dir.placement=ram"), "{ram_state}");
    assert!(
        disk_state.contains("work_dir.placement=disk"),
        "{disk_state}"
    );
    let work_path = state_line(&ram_state, "work_dir.path=")
        .expect("ram work dir in state")
        .trim_start_matches("work_dir.path=")
        .to_string();
    assert!(Path::new(&work_path).starts_with(&ram_base), "{work_path}");
    assert!(
        state_line(&ram_state, "filesystem.1.output=")
            .expect("filesystem output")
            .contains(&ram_base.display().to_string())
    );
    // The RAM copies are gone once the assembly ends.
    assert!(!Path::new(&work_path).exists());
    assert_eq!(ram_summary.archive_path, Some(ram_out.join("disk.img")));
    assert_eq!(disk_summary.archive_path, Some(disk_out.join("disk.img")));
    assert!(
        ram_summary
            .messages
            .iter()
            .any(|message| message.contains("in RAM"))
    );

    for root in [&disk_root, &ram_root, &ram_base] {
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn ram_work_dir_falls_back_to_disk_when_memory_is_short() {
    let root = unique_dir("gaia-assembly-ram-short");
    let ram_base = unique_dir("gaia-assembly-ram-short-base");
    write_ram_test_assets(&root);
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(ram_test_assembly("ram"));

    let summary = stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, || {
        ram_env(&ram_base, 1024 * 1024)
    })
    .expect("fallback assembly");

    let state = summary.state.render();
    assert!(state.contains("work_dir.placement=disk"), "{state}");
    assert!(
        summary
            .messages
            .iter()
            .any(|message| message.contains("building on disk")),
        "{:?}",
        summary.messages
    );
    let out = root.join("out/images");
    assert!(out.join("disk.img").is_file());
    assert!(out.join("boot.cpio").is_file());
    assert!(!ram_base.join("gaia-tester").exists());

    for path in [&root, &ram_base] {
        let _ = fs::remove_dir_all(path);
    }
}
