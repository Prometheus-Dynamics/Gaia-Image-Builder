//! U-Boot boot files on synthesized trees: extlinux.conf entries and their
//! files, `boot.scr` CRCs, and FIT images' data, hashes and configurations.

use super::super::kernel_modules::{Outcome, Severity};
use super::super::test_dtb::Dtb;
use super::*;
use gaia_testkit::TempDir;

fn put(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
    std::fs::write(path, bytes).expect("write");
}

fn run(tree: &Path, arch: Option<TargetArch>) -> Outcome {
    let mut out = Outcome::default();
    check(
        "build",
        &[(tree.to_path_buf(), "boot".into())],
        arch,
        &mut out,
    );
    out
}

fn errors(out: &Outcome) -> Vec<(&'static str, String)> {
    out.findings
        .iter()
        .filter(|finding| finding.severity == Severity::Error)
        .map(|finding| (finding.code, finding.message.clone()))
        .collect()
}

/// An arm64 `Image`: the magic at 0x38.
fn arm64_image() -> Vec<u8> {
    let mut image = vec![0u8; 64];
    image[0x38..0x3c].copy_from_slice(b"ARM\x64");
    image
}

/// A riscv `Image`: the magic at 0x38.
fn riscv_image() -> Vec<u8> {
    let mut image = vec![0u8; 64];
    image[0x38..0x3c].copy_from_slice(b"RSC\x05");
    image
}

fn board_dtb() -> Vec<u8> {
    let mut dtb = Dtb::default();
    dtb.begin("");
    dtb.strings("compatible", &["acme,board"]);
    dtb.end();
    dtb.finish()
}

#[test]
fn a_complete_extlinux_entry_passes_and_paths_resolve_from_the_conf_dir() {
    let scratch = TempDir::new("gaia-uboot");
    let tree = scratch.join("tree");
    put(
        &tree.join("extlinux/extlinux.conf"),
        b"# A/B\ndefault slot-a\nlabel slot-a\n  kernel /Image\n  fdt ../rk3588s-board.dtb\n  \
          initrd /initrd.img\n  fdtoverlays /overlays/a.dtbo /overlays/b.dtbo\n  \
          append root=/dev/dm-0 roothash=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n\
          label recovery\n  localboot 1\n",
    );
    put(&tree.join("Image"), &arm64_image());
    put(&tree.join("rk3588s-board.dtb"), &board_dtb());
    put(&tree.join("initrd.img"), b"cpio");
    put(&tree.join("overlays/a.dtbo"), &board_dtb());
    put(&tree.join("overlays/b.dtbo"), &board_dtb());

    let out = run(&tree, Some(TargetArch::AArch64));

    assert_eq!(errors(&out), []);
}

#[test]
fn missing_files_a_bad_default_roothash_fdt_and_kernel_arch_are_errors() {
    let scratch = TempDir::new("gaia-uboot-broken");
    let tree = scratch.join("tree");
    put(
        &tree.join("boot/extlinux/extlinux.conf"),
        b"DEFAULT nope\nLABEL a\n  LINUX /boot/Image\n  FDT /boot/board.dtb\n  INITRD /boot/missing.img\n  \
          APPEND roothash=xyz\nLABEL b\n  FDTDIR /boot/dtbs\n",
    );
    put(&tree.join("boot/Image"), &riscv_image());
    put(&tree.join("boot/board.dtb"), b"not a device tree");

    let out = run(&tree, Some(TargetArch::AArch64));

    let codes: Vec<&str> = errors(&out).iter().map(|(code, _)| *code).collect();
    assert_eq!(
        codes,
        [
            "uboot_extlinux_default_missing",
            "uboot_extlinux_kernel_invalid",
            "uboot_extlinux_fdt_invalid",
            "uboot_extlinux_file_missing",
            "uboot_extlinux_roothash_invalid",
            "uboot_extlinux_kernel_missing",
            "uboot_extlinux_file_missing",
        ],
        "{:?}",
        errors(&out)
    );
    let messages = errors(&out);
    assert!(
        messages[1]
            .1
            .contains("a riscv64 kernel in a aarch64 image"),
        "{messages:?}"
    );
    assert!(
        messages[3].1.contains("initrd '/boot/missing.img'"),
        "{messages:?}"
    );
}

#[test]
fn a_snapshot_tree_finds_its_kernel_in_the_file_list() {
    let scratch = TempDir::new("gaia-uboot-snapshot");
    let tree = scratch.join("tree");
    put(
        &tree.join("extlinux/extlinux.conf"),
        b"label a\n  kernel /Image\n  initrd /initrd.img\n",
    );
    put(
        &tree.join(BOOT_TREE_FILES_LIST_FOR_TESTS),
        b"Image\nextlinux/extlinux.conf\n",
    );

    let out = run(&tree, Some(TargetArch::AArch64));

    let messages = errors(&out);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(
        messages[0].1.contains("initrd '/initrd.img'"),
        "{messages:?}"
    );
}

const BOOT_TREE_FILES_LIST_FOR_TESTS: &str = FILES_LIST;

/// A legacy script image as `mkimage -T script` writes it.
fn legacy_script(script: &[u8]) -> Vec<u8> {
    let mut data = (script.len() as u32).to_be_bytes().to_vec();
    data.extend_from_slice(&[0; 4]);
    data.extend_from_slice(script);
    let mut header = vec![0u8; LEGACY_HEADER];
    header[0..4].copy_from_slice(&LEGACY_MAGIC.to_be_bytes());
    header[12..16].copy_from_slice(&(data.len() as u32).to_be_bytes());
    header[24..28].copy_from_slice(&crc32(&data).to_be_bytes());
    header[30] = 6;
    let crc = crc32(&header);
    header[4..8].copy_from_slice(&crc.to_be_bytes());
    header.extend_from_slice(&data);
    header
}

#[test]
fn boot_scr_crcs_are_checked() {
    let scratch = TempDir::new("gaia-uboot-scr");
    let good = scratch.join("good");
    put(
        &good.join("boot.scr"),
        &legacy_script(b"sysboot mmc 0:1 any ${scriptaddr} /extlinux/extlinux.conf\n"),
    );
    assert_eq!(errors(&run(&good, None)), []);

    let bad = scratch.join("bad");
    let mut corrupt = legacy_script(b"setenv boot_slot a\n");
    let last = corrupt.len() - 1;
    corrupt[last] ^= 0xff;
    put(&bad.join("boot/boot.scr"), &corrupt);
    let messages = errors(&run(&bad, None));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].0, "uboot_script_invalid");
    assert!(messages[0].1.contains("data CRC"), "{messages:?}");

    let text = scratch.join("text");
    put(
        &text.join("boot.scr"),
        b"setenv bootargs root=/dev/mmcblk0p2\n",
    );
    assert!(errors(&run(&text, None))[0].1.contains("mkimage -T script"));
}

/// A FIT with a kernel (inline data, sha256 and crc32 hashes), an fdt with
/// external data after the tree, and one configuration.
fn fit(kernel: &[u8], kernel_sha256: &[u8], fdt_size: u32, config_fdt: &str) -> Vec<u8> {
    let mut dtb = Dtb::default();
    dtb.begin("");
    dtb.begin("images");
    dtb.begin("kernel");
    dtb.prop("data", kernel);
    dtb.begin("hash-1");
    dtb.strings("algo", &["sha256"]);
    dtb.prop("value", kernel_sha256);
    dtb.end();
    dtb.begin("hash-2");
    dtb.strings("algo", &["crc32"]);
    dtb.prop("value", &crc32(kernel).to_be_bytes());
    dtb.end();
    dtb.end();
    dtb.begin("fdt-1");
    dtb.prop("data-offset", &0u32.to_be_bytes());
    dtb.prop("data-size", &fdt_size.to_be_bytes());
    dtb.end();
    dtb.end();
    dtb.begin("configurations");
    dtb.strings("default", &["conf-1"]);
    dtb.begin("conf-1");
    dtb.strings("kernel", &["kernel"]);
    dtb.strings("fdt", &[config_fdt]);
    dtb.end();
    dtb.end();
    dtb.end();
    let mut bytes = dtb.finish();
    while bytes.len() % 4 != 0 {
        bytes.push(0);
    }
    bytes.extend_from_slice(&[0xdd; 100]);
    bytes
}

#[test]
fn a_fit_with_matching_hashes_and_data_in_the_file_passes() {
    let scratch = TempDir::new("gaia-uboot-fit");
    let tree = scratch.join("tree");
    let kernel = arm64_image();
    put(
        &tree.join("boot/kernel.itb"),
        &fit(&kernel, &Sha256::digest(&kernel), 100, "fdt-1"),
    );

    assert_eq!(errors(&run(&tree, None)), []);
}

#[test]
fn a_fit_with_a_wrong_hash_missing_data_or_a_dangling_config_is_an_error() {
    let scratch = TempDir::new("gaia-uboot-fit-bad");
    let tree = scratch.join("tree");
    let kernel = arm64_image();
    put(
        &tree.join("kernel.itb"),
        &fit(&kernel, &[0u8; 32], 4096, "fdt-2"),
    );

    let messages: Vec<String> = errors(&run(&tree, None))
        .into_iter()
        .map(|(code, message)| {
            assert_eq!(code, "uboot_fit_invalid");
            message
        })
        .collect();

    assert_eq!(messages.len(), 3, "{messages:?}");
    assert!(messages[0].contains("image 'kernel' sha256 does not match"));
    assert!(messages[1].contains("image 'fdt-1' data (4096 bytes at"));
    assert!(messages[2].contains("configuration 'conf-1' fdt 'fdt-2' names no image"));
}
