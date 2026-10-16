//! The `cpio-xz` filesystem: an xz stream with a CRC32 check (the kernel's
//! decoder), the same cpio as `cpio-zstd`, and the same bytes on every run.
//! Split from `tests.rs` for its length.

use super::tests::test_spec;
use super::tests_binaries::{disk_env_panic, state_value};
use super::*;
use gaia_spec::{
    AssemblyFileSpec, AssemblyFilesystemKindSpec, AssemblyFilesystemSpec, AssemblyTreeSpec,
};
use gaia_testkit::TempDir;
use std::fs;
use std::path::Path;
use std::process::Command as HostCommand;

const XZ_MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0x00];
/// The stream flags' check type for CRC32.
const CHECK_CRC32: u8 = 0x01;

fn host_has(tool: &str) -> bool {
    HostCommand::new(tool)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

fn filesystem(id: &str, kind: AssemblyFilesystemKindSpec, output: &str) -> AssemblyFilesystemSpec {
    AssemblyFilesystemSpec {
        free: None,
        id: id.into(),
        kind,
        source_tree: "initramfs".into(),
        output: output.into(),
        size: None,
        deterministic: true,
        compression_level: None,
        publish: false,
        label: None,
        uuid: None,
        journal: None,
    }
}

/// An initramfs tree of one file, with the given filesystems packed from it.
fn initramfs(root: &Path, filesystems: Vec<AssemblyFilesystemSpec>) -> ImageAssemblySpec {
    let text = root.join("config.txt");
    fs::write(&text, "initramfs config\n").expect("config");
    ImageAssemblySpec {
        work_dir: Some("$assembly.work".into()),
        trees: vec![AssemblyTreeSpec {
            id: "initramfs".into(),
            path: "$assembly.work/initramfs".into(),
            strip: false,
        }],
        files: vec![AssemblyFileSpec {
            include: Vec::new(),
            strip: false,
            tree: "initramfs".into(),
            src: Some(text.display().to_string().into()),
            src_glob: None,
            exclude: Vec::new(),
            content: None,
            json: None,
            dest: "etc/config.txt".into(),
            mode: None,
            optional: false,
            preserve_symlink: false,
            allow_missing_libs: false,
        }],
        filesystems,
        ..ImageAssemblySpec::default()
    }
}

/// Stages the filesystems and returns the bytes of the one named `output`.
fn stage_filesystem(root: &Path, filesystems: Vec<AssemblyFilesystemSpec>) -> Vec<u8> {
    let mut spec = test_spec(root);
    spec.image.assembly = Some(initramfs(root, filesystems));
    let summary =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect("filesystem assembly");
    fs::read(state_value(&summary, "filesystem.1.output")).expect("filesystem output")
}

/// The decompressed bytes of `compressed` through the host's `tool -dc`.
fn decompressed(tool: &str, args: &[&str], compressed: &Path) -> Vec<u8> {
    let output = HostCommand::new(tool)
        .args(args)
        .arg(compressed)
        .output()
        .expect("decompressor");
    assert!(output.status.success(), "{tool} failed");
    output.stdout
}

#[cfg(unix)]
#[test]
fn cpio_xz_is_an_xz_stream_with_a_crc32_check_of_the_same_cpio_as_cpio_zstd() {
    if !host_has("xz") || !host_has("zstd") {
        eprintln!("skipping: xz and zstd are needed on the host");
        return;
    }
    let scratch = TempDir::new("gaia-exec-cpio-xz");
    let root = scratch.join("gaia-cpio-xz");
    fs::create_dir_all(&root).expect("test root");
    let xz_out = "$assembly.work/initramfs.cpio.xz";
    let zstd_out = "$assembly.work/initramfs.cpio.zst";
    let mut xz = filesystem("initramfs-xz", AssemblyFilesystemKindSpec::CpioXz, xz_out);
    xz.compression_level = Some(6);
    let zstd = filesystem(
        "initramfs-zstd",
        AssemblyFilesystemKindSpec::CpioZstd,
        zstd_out,
    );

    let first = stage_filesystem(&root, vec![xz.clone(), zstd]);
    let second = stage_filesystem(&root, vec![xz]);

    assert_eq!(&first[..6], &XZ_MAGIC, "xz magic");
    assert_eq!(first[7], CHECK_CRC32, "CRC32 check in the stream flags");
    assert_eq!(first, second, "two runs give the same bytes");
    let xz_path = root.join("build/assembly/initramfs.cpio.xz");
    let zst_path = root.join("build/assembly/initramfs.cpio.zst");
    assert_eq!(
        decompressed("xz", &["-dc"], &xz_path),
        decompressed("zstd", &["-dc", "-q"], &zst_path),
        "the same cpio as cpio-zstd"
    );
}
