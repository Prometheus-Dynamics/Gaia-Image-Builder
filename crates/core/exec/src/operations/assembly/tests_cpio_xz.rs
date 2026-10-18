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
        compressed: Default::default(),
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

/// The LZMA2 dictionary sizes `xz -lvv` reports for a stream, in bytes.
fn xz_dictionaries(compressed: &Path) -> Vec<u64> {
    let output = HostCommand::new("xz")
        .args(["-lvv"])
        .arg(compressed)
        .output()
        .expect("xz -lvv");
    assert!(output.status.success(), "xz -lvv failed");
    let text = String::from_utf8_lossy(&output.stdout);
    // The kernel did not boot a stream whose block header lacks its sizes.
    let sizes_in_headers: Vec<&str> = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("Sizes in headers:"))
        .map(str::trim)
        .collect();
    assert!(
        !sizes_in_headers.is_empty() && sizes_in_headers.iter().all(|value| *value == "Yes"),
        "block headers carry the sizes: {text}"
    );
    text.split("dict=")
        .skip(1)
        .map(|rest| {
            let value: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
            let number: u64 = digits.parse().expect("dict size");
            match &value[digits.len()..] {
                "KiB" => number * 1024,
                "MiB" => number * 1024 * 1024,
                "GiB" => number * 1024 * 1024 * 1024,
                "" | "B" => number,
                other => panic!("unknown dict unit '{other}' in: {text}"),
            }
        })
        .collect()
}

/// Level 9 asks xz for a 64 MiB dictionary, which the kernel cannot
/// allocate while it unpacks an initramfs: the dictionary stays at 1 MiB,
/// or less for a small cpio.
#[cfg(unix)]
#[test]
fn cpio_xz_caps_the_dictionary_at_one_mib_whatever_the_level() {
    if !host_has("xz") {
        eprintln!("skipping: xz is needed on the host");
        return;
    }
    let scratch = TempDir::new("gaia-exec-cpio-xz-dict");
    let root = scratch.join("gaia-cpio-xz-dict");
    fs::create_dir_all(&root).expect("test root");
    let mut xz = filesystem(
        "initramfs-xz",
        AssemblyFilesystemKindSpec::CpioXz,
        "$assembly.work/initramfs.cpio.xz",
    );
    xz.compression_level = Some(9);

    // A cpio larger than 1 MiB; cheap to compress, so the test is quick.
    let big = root.join("big.bin");
    let bytes: Vec<u8> = (0..(3u32 << 19)).map(|i| (i % 251) as u8).collect();
    fs::write(&big, bytes).expect("big file");
    let mut spec = test_spec(&root);
    let mut assembly = initramfs(&root, vec![xz.clone()]);
    assembly.files[0].src = Some(big.display().to_string().into());
    spec.image.assembly = Some(assembly);
    let summary =
        stage_image_assembly_with(&spec, &OperationId::image_assembly(), None, disk_env_panic)
            .expect("filesystem assembly");
    let output = state_value(&summary, "filesystem.1.output");
    // The block header right after the 12-byte stream header: plain `-T2`
    // writes the padded 20-byte header ((0x04 + 1) * 4) that booted, where
    // `--block-size=<cpio size>` wrote a 16-byte one that did not.
    let bytes = fs::read(&output).expect("cpio-xz output");
    assert_eq!(bytes[12], 0x04, "a 20-byte block header");
    let dicts = xz_dictionaries(Path::new(&output));
    assert!(!dicts.is_empty(), "xz -lvv names the dictionary");
    assert!(
        dicts
            .iter()
            .all(|dict| *dict == super::filesystems::CPIO_XZ_MAX_DICT),
        "a cpio over 1 MiB gets a 1 MiB dictionary: {dicts:?}"
    );

    // A small cpio gets a dictionary no larger than itself.
    let small_root = scratch.join("gaia-cpio-xz-dict-small");
    fs::create_dir_all(&small_root).expect("small root");
    let small = stage_filesystem(&small_root, vec![xz]);
    let small_path = small_root.join("build/assembly/initramfs.cpio.xz");
    assert_eq!(&small[..6], &XZ_MAGIC, "xz magic");
    let dicts = xz_dictionaries(&small_path);
    assert!(
        dicts.iter().all(|dict| *dict <= 8 * 1024),
        "a cpio of a few KiB gets a few KiB of dictionary: {dicts:?}"
    );
}

#[test]
fn cpio_xz_filter_caps_the_dictionary_and_keeps_the_preset() {
    use super::filesystems::cpio_xz_filter;
    assert_eq!(
        cpio_xz_filter(Some(9), 64 * 1024 * 1024),
        "--lzma2=preset=9,dict=1048576"
    );
    assert_eq!(cpio_xz_filter(None, 10_000), "--lzma2=preset=6,dict=12288");
    assert_eq!(cpio_xz_filter(Some(0), 0), "--lzma2=preset=0,dict=4096");
}
