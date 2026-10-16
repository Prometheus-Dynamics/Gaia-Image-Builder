//! `ext4` filesystem images of a tree: contents, ownership, reproducibility,
//! sizing and options, checked with the e2fsprogs tools. Skipped where
//! those tools are not installed. Split from `tests.rs` for its length.

use super::*;
use gaia_spec::{
    AssemblyFileSpec, AssemblyFilesystemKindSpec, AssemblySymlinkSpec, AssemblyTreeSpec,
};
use gaia_testkit::TempDir;
use std::fs;
use std::process::Command;

const MIB_BYTES: u64 = 1024 * 1024;

/// The test spec with command timeouts long enough for the e2fsprogs and
/// veritysetup runs on a busy host; the default test timeout is one second.
fn roomy_spec(root: &Path) -> ResolvedBuildSpec {
    let mut spec = test_spec(root);
    spec.policy.providers.buildroot.timeout_seconds = 300;
    spec.policy.providers.starting_point.timeout_seconds = 300;
    spec
}

fn disk_env_panic() -> PlacementEnv {
    panic!("a disk work dir reads no RAM facts")
}

/// The value of one `dumpe2fs -h` field, e.g. `Filesystem volume name:`.
fn dumpe2fs_field(header: &str, name: &str) -> Option<String> {
    header
        .lines()
        .find_map(|line| line.strip_prefix(name))
        .map(|value| value.trim().to_string())
}

fn e2fs_tools_present() -> bool {
    ["mkfs.ext4", "debugfs", "e2fsck", "dumpe2fs"]
        .iter()
        .all(|tool| {
            Command::new(tool)
                .arg("-V")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok()
        })
}

/// The fixture tree's sources under `assets/rootfs`.
fn write_rootfs_assets(root: &Path) {
    let rootfs = root.join("assets/rootfs");
    fs::create_dir_all(rootfs.join("etc")).expect("etc");
    fs::create_dir_all(rootfs.join("bin")).expect("bin");
    fs::create_dir_all(rootfs.join("dir with space")).expect("spaced dir");
    fs::write(rootfs.join("etc/motd"), "hello\n").expect("motd");
    fs::write(rootfs.join("bin/tool"), vec![0x5au8; 9000]).expect("tool");
    fs::write(rootfs.join("dir with space/f e.txt"), "spaced\n").expect("spaced file");
}

fn rootfs_file(src: &str, dest: &str, mode: Option<&str>) -> AssemblyFileSpec {
    AssemblyFileSpec {
        exclude: Vec::new(),
        tree: "rootfs".into(),
        src: Some(src.into()),
        src_glob: None,
        dest: dest.into(),
        mode: mode.map(Into::into),
        optional: false,
        preserve_symlink: false,
        content: None,
        json: None,
    }
}

/// A root filesystem tree of a few files, a directory and a symlink,
/// packed as one ext4 image at `output`.
fn ext4_assembly(
    output: &str,
    size: Option<&str>,
    journal: Option<bool>,
    label: Option<&str>,
    uuid: Option<&str>,
) -> ImageAssemblySpec {
    ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        trees: vec![AssemblyTreeSpec {
            id: "rootfs".into(),
            path: "$assembly.work/rootfs".into(),
        }],
        files: vec![
            rootfs_file("@assets/rootfs/etc/motd", "etc/motd", None),
            rootfs_file("@assets/rootfs/bin/tool", "bin/tool", Some("0755")),
            rootfs_file(
                "@assets/rootfs/dir with space/f e.txt",
                "dir with space/f e.txt",
                None,
            ),
        ],
        symlinks: vec![AssemblySymlinkSpec {
            tree: "rootfs".into(),
            path: "link".into(),
            target: "etc/motd".into(),
        }],
        filesystems: vec![gaia_spec::AssemblyFilesystemSpec {
            free: None,
            id: "rootfs".into(),
            kind: AssemblyFilesystemKindSpec::Ext4,
            source_tree: "rootfs".into(),
            output: output.into(),
            size: size.map(Into::into),
            deterministic: true,
            compression_level: None,
            publish: false,
            label: label.map(Into::into),
            uuid: uuid.map(Into::into),
            journal,
        }],
        ..ImageAssemblySpec::default()
    }
}

fn debugfs(image: &Path, command: &str) -> String {
    let output = Command::new("debugfs")
        .arg("-R")
        .arg(command)
        .arg(image)
        .output()
        .expect("debugfs");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn assert_fsck_clean(image: &Path) {
    let status = Command::new("e2fsck")
        .arg("-fn")
        .arg(image)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("e2fsck");
    assert!(
        status.success(),
        "e2fsck found problems in {}",
        image.display()
    );
}

#[test]
fn ext4_image_holds_the_tree_root_owned_and_reproducible() {
    if !e2fs_tools_present() {
        return;
    }
    let scratch = TempDir::new("gaia-exec-ext4");
    let root = scratch.join("gaia-assembly-ext4");
    write_rootfs_assets(&root);
    let mut spec = roomy_spec(&root);
    spec.image.assembly = Some(ext4_assembly(
        "$assembly.work/rootfs.ext4",
        Some("16M"),
        None,
        Some("rootfs"),
        None,
    ));
    let operation = OperationId::image_assembly();

    let first = stage_image_assembly_with(&spec, &operation, None, disk_env_panic)
        .expect("first ext4 assembly");
    let image = root.join("build/assembly/rootfs.ext4");
    let first_bytes = fs::read(&image).expect("ext4 image");
    assert_eq!(first_bytes.len() as u64, 16 * MIB_BYTES);
    assert_fsck_clean(&image);
    let motd = debugfs(&image, "cat /etc/motd");
    assert_eq!(motd, "hello\n");
    let tool = debugfs(&image, "stat /bin/tool");
    assert!(tool.contains("User:     0   Group:     0"), "{tool}");
    let spaced = debugfs(&image, "cat \"/dir with space/f e.txt\"");
    assert_eq!(spaced, "spaced\n");
    let listing = debugfs(&image, "ls -l /");
    assert!(listing.contains(" link"), "{listing}");
    let state = first.state.render();
    assert!(state.contains("filesystem.1.kind=ext4"), "{state}");
    assert!(state.contains("filesystem.1.tool_version="), "{state}");

    // The same tree gives the same bytes: fixed UUID, hash seed and times,
    // and every entry set to root:root at the epoch.
    let second = stage_image_assembly_with(&spec, &operation, None, disk_env_panic)
        .expect("second ext4 assembly");
    assert_eq!(fs::read(&image).expect("second image"), first_bytes);
    let sha = |summary: &AssemblyStagingSummary| {
        summary
            .state
            .render()
            .lines()
            .find(|line| line.starts_with("filesystem.1.sha256="))
            .map(str::to_string)
    };
    assert_eq!(sha(&first), sha(&second));
}

#[test]
fn ext4_auto_size_fits_the_tree_and_disables_the_journal_on_request() {
    if !e2fs_tools_present() {
        return;
    }
    let scratch = TempDir::new("gaia-exec-ext4-auto");
    let root = scratch.join("gaia-assembly-ext4-auto");
    write_rootfs_assets(&root);
    let mut spec = roomy_spec(&root);
    spec.image.assembly = Some(ext4_assembly(
        "$assembly.work/rootfs.ext4",
        Some("auto"),
        Some(false),
        Some("data"),
        Some("6e2d2b1c-0000-4000-8000-00000000abcd"),
    ));
    let operation = OperationId::image_assembly();
    stage_image_assembly_with(&spec, &operation, None, disk_env_panic)
        .expect("auto-sized ext4 assembly");
    let image = root.join("build/assembly/rootfs.ext4");
    let bytes = fs::metadata(&image).expect("image").len();
    assert_eq!(bytes % MIB_BYTES, 0, "auto size is whole MiB");
    assert_fsck_clean(&image);
    assert_eq!(debugfs(&image, "cat /etc/motd"), "hello\n");
    let features = debugfs(&image, "features");
    assert!(!features.contains("has_journal"), "{features}");
    let stats = Command::new("dumpe2fs")
        .arg("-h")
        .arg(&image)
        .output()
        .expect("dumpe2fs");
    let stats = String::from_utf8_lossy(&stats.stdout);
    assert_eq!(
        dumpe2fs_field(&stats, "Filesystem volume name:").as_deref(),
        Some("data"),
        "{stats}"
    );
    assert_eq!(
        dumpe2fs_field(&stats, "Filesystem UUID:").as_deref(),
        Some("6e2d2b1c-0000-4000-8000-00000000abcd"),
        "{stats}"
    );
}

#[test]
fn ext4_image_built_in_ram_is_published_as_the_disk_build_publishes_it() {
    if !e2fs_tools_present() {
        return;
    }
    let scratch = TempDir::new("gaia-exec-ext4-ram");
    let disk_root = scratch.join("gaia-assembly-ext4-ram-disk");
    let ram_root = scratch.join("gaia-assembly-ext4-ram-ram");
    let ram_base = scratch.join("gaia-assembly-ext4-ram-base");
    for dir in [&disk_root, &ram_root, &ram_base] {
        fs::create_dir_all(dir).expect("test root");
    }
    write_rootfs_assets(&disk_root);
    write_rootfs_assets(&ram_root);
    let mut disk_assembly =
        ext4_assembly("$assembly.work/rootfs.ext4", Some("16M"), None, None, None);
    disk_assembly.filesystems[0].publish = true;
    let mut ram_assembly = disk_assembly.clone();
    ram_assembly.work_dir = Some("ram".into());
    let mut disk_spec = roomy_spec(&disk_root);
    disk_spec.image.assembly = Some(disk_assembly);
    let mut ram_spec = roomy_spec(&ram_root);
    ram_spec.image.assembly = Some(ram_assembly);
    let operation = OperationId::image_assembly();

    let disk_summary = stage_image_assembly_with(&disk_spec, &operation, None, disk_env_panic)
        .expect("disk ext4 assembly");
    let ram_summary = stage_image_assembly_with(&ram_spec, &operation, None, || PlacementEnv {
        ram_base: ram_base.clone(),
        user: "tester".into(),
        memory_available: Some(64 * 1024 * MIB_BYTES),
        tmpfs_available: Some(64 * 1024 * MIB_BYTES),
    })
    .expect("ram ext4 assembly");

    let published = |root: &Path| fs::read(root.join("out/images/rootfs.ext4")).expect("published");
    assert_eq!(published(&disk_root), published(&ram_root));
    let ram_state = ram_summary.state.render();
    assert!(ram_state.contains("work_dir.placement=ram"), "{ram_state}");
    let digest = |summary: &AssemblyStagingSummary| {
        summary
            .state
            .render()
            .lines()
            .find(|line| line.starts_with("filesystem.1.sha256="))
            .map(str::to_string)
    };
    assert_eq!(digest(&disk_summary), digest(&ram_summary));
}

#[test]
fn ext4_auto_bytes_cover_the_content_in_whole_mib() {
    let scratch = TempDir::new("gaia-exec-ext4-size");
    let tree = scratch.join("tree");
    fs::create_dir_all(tree.join("a")).expect("dir");
    fs::write(tree.join("a/big"), vec![1u8; 3 * 1024 * 1024 + 1]).expect("big");
    fs::write(tree.join("small"), "x").expect("small");
    let bytes = ext4_auto_bytes(&tree, 0).expect("auto bytes");
    let content = 3 * 1024 * 1024 + 1 + 4096 + 4096 + 4096 + 4096;
    assert_eq!(bytes % MIB_BYTES, 0);
    assert!(
        bytes >= content + 8 * MIB_BYTES,
        "{bytes} for {content} of content"
    );
    assert!(bytes < content + 64 * MIB_BYTES, "{bytes} is far too large");
}

#[test]
fn ext4_rejects_a_name_debugfs_cannot_set() {
    let scratch = TempDir::new("gaia-exec-ext4-name");
    let tree = scratch.join("tree");
    fs::create_dir_all(&tree).expect("tree");
    fs::write(tree.join("quo\"te"), "x").expect("file");
    let error = ext4_normalize_script(&tree).expect_err("quote in name");
    assert!(error.contains("cannot set"), "{error}");
}
