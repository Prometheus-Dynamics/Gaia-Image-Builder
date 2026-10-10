//! `cpio-zstd` filesystem images and published filesystem images, in disk
//! and RAM work dirs. Split from `tests.rs` for its length.

use super::*;
use gaia_spec::{AssemblyFileSpec, AssemblyTreeSpec};
use std::fs;
use std::process::Command;

const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

fn tool_runs(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

fn write_assets(root: &Path) {
    let assets = root.join("assets");
    fs::create_dir_all(&assets).expect("assets");
    fs::write(assets.join("config.txt"), "initramfs config\n").expect("config");
}

/// A boot tree of one file, packed as a `cpio-zstd` image: in the work dir
/// (`output`), optionally published.
fn cpio_zstd_assembly(work_dir: &str, output: &str, publish: bool) -> ImageAssemblySpec {
    ImageAssemblySpec {
        work_dir: Some(work_dir.into()),
        trees: vec![AssemblyTreeSpec {
            id: "initramfs".into(),
            path: "$assembly.work/initramfs".into(),
        }],
        files: vec![AssemblyFileSpec {
            tree: "initramfs".into(),
            src: Some("@assets/config.txt".into()),
            src_glob: None,
            dest: "etc/config.txt".into(),
            mode: None,
            optional: false,
            preserve_symlink: false,
        }],
        filesystems: vec![gaia_spec::AssemblyFilesystemSpec {
            id: "initramfs".into(),
            kind: gaia_spec::AssemblyFilesystemKindSpec::CpioZstd,
            source_tree: "initramfs".into(),
            output: output.into(),
            size: None,
            deterministic: true,
            compression_level: Some(3),
            publish,
        }],
        ..ImageAssemblySpec::default()
    }
}

fn disk_env_panic() -> PlacementEnv {
    panic!("a disk work dir reads no RAM facts")
}

fn ram_env(base: &Path) -> PlacementEnv {
    PlacementEnv {
        ram_base: base.to_path_buf(),
        user: "tester".into(),
        memory_available: Some(64 * 1024 * 1024 * 1024),
        tmpfs_available: Some(64 * 1024 * 1024 * 1024),
    }
}

/// The decompressed bytes of a zstd file, as a cpio listing.
fn cpio_listing(image: &Path, scratch: &Path) -> String {
    let decoded = Command::new("zstd")
        .args(["-dc", "-q"])
        .arg(image)
        .output()
        .expect("zstd -dc");
    assert!(decoded.status.success(), "{decoded:?}");
    assert!(decoded.stdout.starts_with(b"070701"), "not a newc cpio");
    let cpio = scratch.join("decoded.cpio");
    fs::write(&cpio, &decoded.stdout).expect("decoded cpio");
    let listing = Command::new("cpio")
        .args(["-it", "--quiet", "-F"])
        .arg(&cpio)
        .output()
        .expect("cpio -it");
    assert!(listing.status.success(), "{listing:?}");
    String::from_utf8_lossy(&listing.stdout).into_owned()
}

#[test]
fn cpio_zstd_packs_the_tree_as_a_deterministic_zstd_newc_archive() {
    if !tool_runs("zstd") || !tool_runs("cpio") {
        return;
    }
    let root = unique_dir("gaia-assembly-cpio-zstd");
    write_assets(&root);
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(cpio_zstd_assembly(
        "$assembly.work",
        "$assembly.work/initramfs.cpio.zst",
        false,
    ));
    let operation = OperationId::image_assembly();

    let first = stage_image_assembly_with(&spec, &operation, None, disk_env_panic)
        .expect("first cpio-zstd assembly");
    let image = root.join("build/assembly/initramfs.cpio.zst");
    let first_bytes = fs::read(&image).expect("cpio-zstd image");
    assert_eq!(&first_bytes[..4], &ZSTD_MAGIC);
    let listing = cpio_listing(&image, &root);
    assert!(listing.contains("etc/config.txt"), "{listing}");
    let state = first.state.render();
    assert!(state.contains("filesystem.1.kind=cpio-zstd"), "{state}");
    assert!(state.contains("filesystem.1.tool_version="), "{state}");
    // The published image is nothing extra: no disk output, no publish.
    assert!(first.disk_images.is_empty());

    // The same tree packs to the same bytes: no timestamps or thread counts.
    let second = stage_image_assembly_with(&spec, &operation, None, disk_env_panic)
        .expect("second cpio-zstd assembly");
    assert_eq!(fs::read(&image).expect("second image"), first_bytes);
    assert_eq!(
        first
            .state
            .render()
            .lines()
            .find(|line| line.starts_with("filesystem.1.sha256=")),
        second
            .state
            .render()
            .lines()
            .find(|line| line.starts_with("filesystem.1.sha256="))
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn published_filesystem_image_is_copied_to_the_image_output_dir() {
    if !tool_runs("zstd") || !tool_runs("cpio") {
        return;
    }
    let root = unique_dir("gaia-assembly-publish-disk");
    write_assets(&root);
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(cpio_zstd_assembly(
        "$assembly.work",
        "$assembly.work/initramfs.cpio.zst",
        true,
    ));

    let summary =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect("published assembly");

    let published = root.join("out/images/initramfs.cpio.zst");
    assert_eq!(
        fs::read(&published).expect("published image"),
        fs::read(root.join("build/assembly/initramfs.cpio.zst")).expect("work image")
    );
    assert_eq!(summary.disk_images, vec![published.clone()]);
    assert!(summary.archive_path.is_none());
    let state = summary.state.render();
    assert!(
        state.contains(&format!("filesystem.1.published={}", published.display())),
        "{state}"
    );
    assert!(
        summary
            .messages
            .iter()
            .any(|message| message.starts_with("published assembly filesystem 'initramfs'")),
        "{:?}",
        summary.messages
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn published_filesystem_image_already_in_the_output_dir_is_not_copied_onto_itself() {
    if !tool_runs("zstd") || !tool_runs("cpio") {
        return;
    }
    let root = unique_dir("gaia-assembly-publish-in-place");
    write_assets(&root);
    let mut spec = test_spec(&root);
    spec.image.assembly = Some(cpio_zstd_assembly(
        "$assembly.work",
        "$provider.images/initramfs.cpio.zst",
        true,
    ));

    let summary =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect("in-place publish");

    let image = root.join("out/images/initramfs.cpio.zst");
    assert!(cpio_listing(&image, &root).contains("etc/config.txt"));
    assert_eq!(summary.disk_images, vec![image]);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn published_filesystem_image_in_ram_is_published_as_the_same_bytes() {
    if !tool_runs("zstd") || !tool_runs("cpio") {
        return;
    }
    let disk_root = unique_dir("gaia-assembly-publish-ram-disk");
    let ram_root = unique_dir("gaia-assembly-publish-ram-ram");
    let ram_base = unique_dir("gaia-assembly-publish-ram-base");
    write_assets(&disk_root);
    write_assets(&ram_root);
    let mut disk_spec = test_spec(&disk_root);
    disk_spec.image.assembly = Some(cpio_zstd_assembly(
        "disk",
        "$assembly.work/initramfs.cpio.zst",
        true,
    ));
    let mut ram_spec = test_spec(&ram_root);
    ram_spec.image.assembly = Some(cpio_zstd_assembly(
        "ram",
        "$assembly.work/initramfs.cpio.zst",
        true,
    ));
    let operation = OperationId::image_assembly();

    let disk_summary = stage_image_assembly_with(&disk_spec, &operation, None, disk_env_panic)
        .expect("disk published");
    let ram_summary = stage_image_assembly_with(&ram_spec, &operation, None, || ram_env(&ram_base))
        .expect("ram published");

    let disk_image = disk_root.join("out/images/initramfs.cpio.zst");
    let ram_image = ram_root.join("out/images/initramfs.cpio.zst");
    assert_eq!(
        fs::read(&disk_image).expect("disk published"),
        fs::read(&ram_image).expect("ram published"),
        "the published copy is the same bytes either way"
    );
    assert_eq!(ram_summary.disk_images, vec![ram_image.clone()]);
    assert_eq!(disk_summary.disk_images, vec![disk_image]);
    let ram_state = ram_summary.state.render();
    assert!(ram_state.contains("work_dir.placement=ram"), "{ram_state}");
    // The RAM image is an intermediate: gone with the RAM copies below.
    let work_line = ram_state
        .lines()
        .find(|line| line.starts_with("work_dir.path="))
        .expect("ram work dir");
    let work = Path::new(work_line.trim_start_matches("work_dir.path="));
    assert!(work.starts_with(&ram_base), "{work:?}");
    assert!(!work.exists());
    assert_eq!(
        ram_summary
            .state
            .render()
            .lines()
            .find(|line| line.starts_with("filesystem.1.sha256=")),
        disk_summary
            .state
            .render()
            .lines()
            .find(|line| line.starts_with("filesystem.1.sha256="))
    );
    for path in [&disk_root, &ram_root, &ram_base] {
        let _ = fs::remove_dir_all(path);
    }
}
