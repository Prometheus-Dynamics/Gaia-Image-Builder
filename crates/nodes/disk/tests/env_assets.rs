//! The K230D AI-CAM's U-Boot environment blobs, committed under
//! examples/boards/k230d-ai-cam/assets: the writer's output for the text beside them.
//!
//! Regenerate both after editing `u-boot-env.txt`:
//!   cargo test -p gaia-disk --test env_assets -- --ignored regenerate
//! The blobs are 0x2000 bytes each, the redundant layout (CRC-32 LE over the data,
//! then one flag byte, then the data), as U-Boot's SYS_REDUNDAND_ENVIRONMENT reads.

use gaia_disk::{UbootVar, parse_env_text, uboot_env_image};
use std::path::{Path, PathBuf};

const SIZE: u64 = 0x2000;

fn assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../examples/boards/k230d-ai-cam/assets")
}

fn text_vars() -> Vec<UbootVar> {
    let text = std::fs::read_to_string(assets().join("u-boot-env.txt")).expect("the env text");
    parse_env_text(&text).expect("parses")
}

/// The blob both copies are: the writer's redundant image of the text.
fn expected() -> Vec<u8> {
    uboot_env_image(&text_vars(), SIZE, true).expect("writes")
}

/// CRC-32 (IEEE, reflected), as U-Boot's crc32() computes it.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[test]
fn the_committed_copies_are_the_writers_output() {
    let want = expected();
    for name in ["u-boot-env.bin", "u-boot-env-redund.bin"] {
        let got = std::fs::read(assets().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(got.len(), SIZE as usize, "{name} is 0x2000 bytes");
        assert!(got == want, "{name} differs from the writer's output of u-boot-env.txt: regenerate it");
    }
}

#[test]
fn the_blobs_parse_back_as_u_boot_reads_them() {
    let blob = std::fs::read(assets().join("u-boot-env.bin")).expect("blob");
    let stored_crc = u32::from_le_bytes([blob[0], blob[1], blob[2], blob[3]]);
    assert_eq!(blob[4], 1, "the flag byte of the active copy");
    let data = &blob[5..];
    assert_eq!(crc32(data), stored_crc, "the CRC covers the data after the flag");
    // Variables: `name=value` NUL-separated, ended by an empty string, then 0xff.
    let mut parsed = Vec::new();
    for entry in data.split(|&b| b == 0) {
        if entry.is_empty() {
            break;
        }
        let text = std::str::from_utf8(entry).expect("text");
        let (name, value) = text.split_once('=').expect("name=value");
        parsed.push(UbootVar { name: name.into(), value: value.into() });
    }
    let want = text_vars();
    assert_eq!(parsed.len(), want.len());
    for (got, want) in parsed.iter().zip(&want) {
        assert_eq!((&got.name, &got.value), (&want.name, &want.value));
    }
}

#[test]
#[ignore = "rewrites the committed blobs from u-boot-env.txt"]
fn regenerate() {
    let image = expected();
    for name in ["u-boot-env.bin", "u-boot-env-redund.bin"] {
        std::fs::write(assets().join(name), &image).expect("write");
    }
}
