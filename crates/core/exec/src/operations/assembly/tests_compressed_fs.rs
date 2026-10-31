//! `squashfs` and `erofs` images of a tree: the tool arguments, and with
//! the real tools the contents, ownership and reproducibility. The tool
//! tests skip with a message where `mksquashfs`/`unsquashfs` or
//! `mkfs.erofs`/`fsck.erofs` are not installed (`GAIA_TEST_MKFS_EROFS` and
//! `GAIA_TEST_FSCK_EROFS` name them elsewhere, e.g. in a Buildroot host dir).

use super::tests_verity::{provide_veritysetup, real_veritysetup};
use super::*;
use gaia_spec::{
    AssemblyCompressedFsSpec, AssemblyFileSpec, AssemblyFilesystemKindSpec, AssemblyFilesystemSpec,
    AssemblySymlinkSpec, AssemblyTransformKindSpec, AssemblyTransformSpec, AssemblyTreeSpec,
};
use gaia_testkit::TempDir;
use std::ffi::OsString;
use std::fs;
use std::process::Command;

fn roomy_spec(root: &Path) -> ResolvedBuildSpec {
    let mut spec = test_spec(root);
    spec.policy.providers.buildroot.timeout_seconds = 300;
    spec.policy.providers.starting_point.timeout_seconds = 300;
    spec
}

fn disk_env_panic() -> PlacementEnv {
    panic!("a disk work dir reads no RAM facts")
}

/// A tool from `env` (a path) or PATH that runs with `version_arg`.
fn real_tool(env: &str, name: &str, version_arg: &str) -> Option<PathBuf> {
    let candidate = std::env::var_os(env).map(PathBuf::from).or_else(|| {
        std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(name))
                .find(|candidate| candidate.is_file())
        })
    })?;
    Command::new(&candidate)
        .arg(version_arg)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()
        .map(|_| candidate)
}

/// Puts a tool where the assembly finds a Buildroot provider's host tools.
fn provide_tool(root: &Path, name: &str, binary: &Path) {
    let bin = root.join("build/image/buildroot-output/host/bin");
    fs::create_dir_all(&bin).expect("provider host bin");
    std::os::unix::fs::symlink(binary, bin.join(name)).expect("tool link");
}

fn write_assets(root: &Path) {
    let assets = root.join("assets");
    fs::create_dir_all(&assets).expect("assets");
    fs::write(assets.join("motd"), "hello\n").expect("motd");
    // Compressible and repeated, so dedupe and fragments have work to do.
    fs::write(assets.join("tool"), b"gaia ".repeat(20_000)).expect("tool");
    fs::write(assets.join("copy"), b"gaia ".repeat(20_000)).expect("copy");
}

fn tree_file(src: &str, dest: &str, mode: Option<&str>) -> AssemblyFileSpec {
    AssemblyFileSpec {
        include: Vec::new(),
        strip: false,
        exclude: Vec::new(),
        tree: "rootfs".into(),
        src: Some(src.into()),
        src_glob: None,
        dest: dest.into(),
        mode: mode.map(Into::into),
        dir_mode: None,
        optional: false,
        src_dir: None,
        preserve_symlink: false,
        allow_missing_libs: false,
        content: None,
        json: None,
    }
}

fn filesystem(
    kind: AssemblyFilesystemKindSpec,
    output: &str,
    compressed: AssemblyCompressedFsSpec,
) -> AssemblyFilesystemSpec {
    AssemblyFilesystemSpec {
        id: "rootfs".into(),
        kind,
        source_tree: "rootfs".into(),
        output: output.into(),
        size: None,
        deterministic: true,
        compression_level: None,
        publish: true,
        label: None,
        uuid: None,
        journal: None,
        free: None,
        compressed,
        fat_size: None,
        reserved_percent: None,
    }
}

fn assembly(filesystem: AssemblyFilesystemSpec) -> ImageAssemblySpec {
    ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        trees: vec![AssemblyTreeSpec {
            strip: false,
            id: "rootfs".into(),
            path: "$assembly.work/rootfs".into(),
        }],
        files: vec![
            tree_file("@assets/motd", "etc/motd", None),
            tree_file("@assets/tool", "usr/bin/tool", Some("0755")),
            tree_file("@assets/copy", "usr/share/copy", None),
        ],
        symlinks: vec![AssemblySymlinkSpec {
            tree: "rootfs".into(),
            path: "motd".into(),
            target: "etc/motd".into(),
        }],
        filesystems: vec![filesystem],
        ..ImageAssemblySpec::default()
    }
}

fn state_value(summary: &AssemblyStagingSummary, key: &str) -> String {
    summary
        .state
        .render()
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")).map(str::to_string))
        .unwrap_or_else(|| panic!("state has no {key}"))
}

/// The files of an unpacked tree, by path, with their contents (a
/// symlink's target), for comparing an unpacked image with its tree.
fn tree_listing(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut listing = Vec::new();
    for (name, path) in super::filesystems::collect_archive_entries(root).expect("walk tree") {
        let metadata = fs::symlink_metadata(&path).expect("entry");
        let content = if metadata.file_type().is_symlink() {
            fs::read_link(&path)
                .expect("link")
                .to_string_lossy()
                .into_owned()
                .into_bytes()
        } else if metadata.is_file() {
            fs::read(&path).expect("file")
        } else {
            Vec::new()
        };
        listing.push((name, content));
    }
    listing
}

fn args_text(args: &[OsString]) -> Vec<String> {
    args.iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn squashfs_arguments_pin_owner_times_and_compressor() {
    let mut spec = filesystem(
        AssemblyFilesystemKindSpec::Squashfs,
        "$assembly.work/root.squashfs",
        AssemblyCompressedFsSpec {
            compression: Some("zstd".into()),
            block_size: Some(65536),
            ..AssemblyCompressedFsSpec::default()
        },
    );
    spec.compression_level = Some(19);
    let args =
        args_text(&squashfs_args(&spec, Path::new("/tree"), Path::new("/out.img")).expect("args"));
    assert_eq!(&args[..2], ["/tree", "/out.img"]);
    for expected in [
        "-noappend",
        "-all-root",
        "-no-xattrs",
        "-reproducible",
        "zstd",
        "19",
        "65536",
    ] {
        assert!(
            args.iter().any(|arg| arg == expected),
            "{expected}: {args:?}"
        );
    }
    let mkfs_time = args
        .iter()
        .position(|arg| arg == "-mkfs-time")
        .expect("time");
    assert_eq!(args[mkfs_time + 1], "0");

    // gzip is the default; an unknown compressor is refused.
    spec.compressed.compression = None;
    spec.compression_level = None;
    let args = args_text(&squashfs_args(&spec, Path::new("/t"), Path::new("/o")).expect("args"));
    let comp = args.iter().position(|arg| arg == "-comp").expect("comp");
    assert_eq!(args[comp + 1], "gzip");
    spec.compressed.compression = Some("lzma".into());
    assert!(squashfs_args(&spec, Path::new("/t"), Path::new("/o")).is_err());
}

#[test]
fn erofs_arguments_pin_owner_times_and_uuid() {
    let mut spec = filesystem(
        AssemblyFilesystemKindSpec::Erofs,
        "$assembly.work/root.erofs",
        AssemblyCompressedFsSpec {
            compression: Some("lzma".into()),
            block_size: Some(4096),
            pcluster_size: Some(65536),
            dedupe: true,
            fragments: false,
            ztailpacking: true,
        },
    );
    spec.compression_level = Some(6);
    spec.label = Some("root".into());
    let args =
        args_text(&erofs_args(&spec, Path::new("/tree"), Path::new("/out.img")).expect("args"));
    let tail: Vec<&str> = args.iter().rev().take(2).map(String::as_str).collect();
    assert_eq!(tail, ["/tree", "/out.img"], "output, then the tree");
    for expected in [
        "-zlzma,6",
        "-C65536",
        "dedupe,ztailpacking",
        "-T0",
        "--all-root",
        "-x-1",
        "root",
    ] {
        assert!(
            args.iter().any(|arg| arg == expected),
            "{expected}: {args:?}"
        );
    }
    let uuid = &args[args.iter().position(|arg| arg == "-U").expect("uuid") + 1];
    assert_eq!(uuid.len(), 36, "{uuid}");
    // The derived UUID follows the id, not the run.
    let again = args_text(&erofs_args(&spec, Path::new("/t"), Path::new("/o")).expect("args"));
    assert!(again.contains(uuid));

    // lz4hc is the default; none writes no -z at all.
    spec.compressed = AssemblyCompressedFsSpec::default();
    spec.compression_level = None;
    let args = args_text(&erofs_args(&spec, Path::new("/t"), Path::new("/o")).expect("args"));
    assert!(args.iter().any(|arg| arg == "-zlz4hc"), "{args:?}");
    spec.compressed.compression = Some("none".into());
    let args = args_text(&erofs_args(&spec, Path::new("/t"), Path::new("/o")).expect("args"));
    assert!(!args.iter().any(|arg| arg.starts_with("-z")), "{args:?}");
}

#[test]
fn squashfs_image_unpacks_to_the_tree_root_owned_and_reproducible() {
    let (Some(mksquashfs), Some(unsquashfs)) = (
        real_tool("GAIA_TEST_MKSQUASHFS", "mksquashfs", "-version"),
        real_tool("GAIA_TEST_UNSQUASHFS", "unsquashfs", "-version"),
    ) else {
        eprintln!("skipped: no mksquashfs/unsquashfs (set GAIA_TEST_MKSQUASHFS)");
        return;
    };
    let scratch = TempDir::new("gaia-exec-squashfs");
    let root = scratch.join("gaia-assembly-squashfs");
    write_assets(&root);
    provide_tool(&root, "mksquashfs", &mksquashfs);
    let mut spec = roomy_spec(&root);
    let mut squashfs = filesystem(
        AssemblyFilesystemKindSpec::Squashfs,
        "$assembly.work/rootfs.squashfs",
        AssemblyCompressedFsSpec {
            compression: Some("xz".into()),
            block_size: Some(131072),
            ..AssemblyCompressedFsSpec::default()
        },
    );
    squashfs.publish = true;
    spec.image.assembly = Some(assembly(squashfs));
    let operation = OperationId::image_assembly();

    let first = stage_image_assembly_with(&spec, &operation, None, disk_env_panic)
        .expect("first squashfs assembly");
    let image = root.join("build/assembly/rootfs.squashfs");
    let bytes = fs::read(&image).expect("squashfs image");
    assert_eq!(&bytes[..4], b"hsqs", "squashfs magic");
    assert_eq!(
        state_value(&first, "filesystem.1.bytes"),
        bytes.len().to_string()
    );
    assert_eq!(state_value(&first, "filesystem.1.kind"), "squashfs");
    assert!(state_value(&first, "filesystem.1.tool").contains("mksquashfs"));
    assert_eq!(
        fs::read(root.join("out/images/rootfs.squashfs")).expect("published copy"),
        bytes
    );

    // Every entry is root:root with the epoch as its time.
    let listing = Command::new(&unsquashfs)
        .env("TZ", "UTC")
        .arg("-lln")
        .arg(&image)
        .output()
        .expect("unsquashfs -lln");
    let listing = String::from_utf8_lossy(&listing.stdout);
    let entries: Vec<&str> = listing
        .lines()
        .filter(|line| line.starts_with(['-', 'd', 'l']))
        .collect();
    assert!(entries.len() >= 6, "{listing}");
    for line in &entries {
        assert!(line.contains(" 0/0 "), "not root-owned: {line}");
        assert!(
            line.contains("1970-01-01 00:00"),
            "not at the epoch: {line}"
        );
    }

    let unpacked = scratch.join("unpacked");
    let extract = Command::new(&unsquashfs)
        .args(["-no-xattrs", "-d"])
        .arg(&unpacked)
        .arg(&image)
        .output()
        .expect("unsquashfs");
    assert!(extract.status.success(), "{extract:?}");
    assert_eq!(
        tree_listing(&unpacked),
        tree_listing(&root.join("build/assembly/rootfs"))
    );

    let second = stage_image_assembly_with(&spec, &operation, None, disk_env_panic)
        .expect("second squashfs assembly");
    assert_eq!(fs::read(&image).expect("second image"), bytes);
    assert_eq!(
        state_value(&second, "filesystem.1.sha256"),
        state_value(&first, "filesystem.1.sha256")
    );
}

#[test]
fn erofs_image_extracts_to_the_tree_and_is_reproducible() {
    let (Some(mkfs), Some(fsck)) = (
        real_tool("GAIA_TEST_MKFS_EROFS", "mkfs.erofs", "--version"),
        real_tool("GAIA_TEST_FSCK_EROFS", "fsck.erofs", "--version"),
    ) else {
        eprintln!("skipped: no mkfs.erofs/fsck.erofs (set GAIA_TEST_MKFS_EROFS)");
        return;
    };
    for (index, (compression, level, layout)) in [
        (None, None, false),
        (Some("lz4hc"), Some(9), true),
        (Some("none"), None, false),
    ]
    .into_iter()
    .enumerate()
    {
        let scratch = TempDir::new("gaia-exec-erofs");
        let root = scratch.join(format!("gaia-assembly-erofs-{index}"));
        write_assets(&root);
        provide_tool(&root, "mkfs.erofs", &mkfs);
        let mut spec = roomy_spec(&root);
        let mut erofs = filesystem(
            AssemblyFilesystemKindSpec::Erofs,
            "$assembly.work/rootfs.erofs",
            AssemblyCompressedFsSpec {
                compression: compression.map(Into::into),
                pcluster_size: layout.then_some(65536),
                dedupe: layout,
                fragments: layout,
                ztailpacking: layout,
                ..AssemblyCompressedFsSpec::default()
            },
        );
        erofs.compression_level = level;
        erofs.label = Some("rootfs".into());
        spec.image.assembly = Some(assembly(erofs));
        let operation = OperationId::image_assembly();

        let first = stage_image_assembly_with(&spec, &operation, None, disk_env_panic)
            .expect("first erofs assembly");
        let image = root.join("build/assembly/rootfs.erofs");
        let bytes = fs::read(&image).expect("erofs image");
        // The superblock magic, little-endian, at 1024.
        assert_eq!(&bytes[1024..1028], &0xe0f5_e1e2u32.to_le_bytes());
        assert_eq!(state_value(&first, "filesystem.1.kind"), "erofs");

        let unpacked = scratch.join("unpacked");
        let extract = Command::new(&fsck)
            .arg(format!("--extract={}", unpacked.display()))
            .arg(&image)
            .output()
            .expect("fsck.erofs --extract");
        assert!(extract.status.success(), "{extract:?}");
        assert_eq!(
            tree_listing(&unpacked),
            tree_listing(&root.join("build/assembly/rootfs"))
        );

        stage_image_assembly_with(&spec, &operation, None, disk_env_panic)
            .expect("second erofs assembly");
        assert_eq!(
            fs::read(&image).expect("second image"),
            bytes,
            "{compression:?}"
        );
    }
}

/// A squashfs image is a verity transform's data like any file: the
/// transform runs after the filesystem and covers exactly its bytes.
#[test]
fn squashfs_image_is_the_data_of_a_verity_transform() {
    let (Some(mksquashfs), Some(veritysetup)) = (
        real_tool("GAIA_TEST_MKSQUASHFS", "mksquashfs", "-version"),
        real_veritysetup(),
    ) else {
        eprintln!("skipped: no mksquashfs/veritysetup (set GAIA_TEST_VERITYSETUP)");
        return;
    };
    let scratch = TempDir::new("gaia-exec-squashfs-verity");
    let root = scratch.join("gaia-assembly-squashfs-verity");
    write_assets(&root);
    provide_tool(&root, "mksquashfs", &mksquashfs);
    provide_veritysetup(&root, &veritysetup);
    let mut spec = roomy_spec(&root);
    let mut assembly = assembly(filesystem(
        AssemblyFilesystemKindSpec::Squashfs,
        "$assembly.work/rootfs.squashfs",
        AssemblyCompressedFsSpec::default(),
    ));
    // Declared before the filesystem it reads, so the order comes from
    // the paths, not the declaration.
    assembly.transforms = vec![AssemblyTransformSpec {
        kind: AssemblyTransformKindSpec::Verity,
        src: Some("$assembly.work/rootfs.squashfs".into()),
        dest: "$assembly.work/rootfs.verity".into(),
        deterministic: true,
        level: None,
        hash_dest: None,
        data_block_size: None,
        hash_block_size: None,
        long: false,
        env_size: None,
        redundant: false,
        salt: None,
        uuid: None,
    }];
    spec.image.assembly = Some(assembly);
    let summary =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect("squashfs verity assembly");
    let data = fs::read(root.join("build/assembly/rootfs.squashfs")).expect("data");
    let verity = fs::read(root.join("build/assembly/rootfs.verity")).expect("verity");
    assert_eq!(&verity[..data.len()], &data[..]);
    let offset: usize = state_value(&summary, "verity.1.hash_offset")
        .parse()
        .expect("offset");
    assert_eq!(offset, data.len().div_ceil(4096) * 4096);
}
