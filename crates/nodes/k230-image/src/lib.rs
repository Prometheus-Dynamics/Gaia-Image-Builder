//! The first-stage image the K230 boot ROM reads: a 528-byte header, then
//! the payload. Read by the ROM, not by Gaia, so the format is the ROM's.
//!
//! Layout, as `firmware_gen_no_securiy.py -n` writes it and as the SDK's
//! images carry it (checked on the probe image's SPL and U-Boot blobs):
//!
//! | bytes  | field                                                    |
//! |--------|----------------------------------------------------------|
//! | 0..4   | `K230`                                                   |
//! | 4..8   | payload length, u32 little-endian                        |
//! | 8..12  | type, u32 little-endian, 0 (no encryption)               |
//! | 12..44 | SHA-256 of the payload                                   |
//! | 44..528| zeros                                                    |
//! | 528..  | payload                                                  |
//!
//! The payload is a 4-byte zero version word followed by the data. The SPL
//! skips that word (`k230_img.c`: the image header is at `plain_addr + 4`).
//!
//! The second-stage U-Boot is a uImage whose gzip stream has its CM byte
//! (offset 2) set to 0x09 (the "priv gzip" marker). The SPL's `gunzip`
//! (`arch/riscv/cpu/k230/unzip.c`) sets it back to 0x08 in place, then has
//! the hardware gzip engine inflate the stream, using the ISIZE trailer as
//! the output length. A stock tool refuses the marker; the stream is an
//! ordinary deflate stream with a CRC-32 and ISIZE trailer.

use std::fmt;
use std::io::Write;

use sha2::{Digest, Sha256};

/// The magic at offset 0.
pub const MAGIC: &[u8; 4] = b"K230";
/// The header's length, and the payload's offset in the image.
pub const HEADER_LEN: usize = 528;
/// The zero version word the SPL skips at the start of the payload.
pub const VERSION_LEN: usize = 4;
/// The gzip CM byte the SDK's SPL reads as the priv marker.
pub const PRIV_GZIP_CM: u8 = 0x09;
/// Deflate, the CM byte a stock gzip writes.
pub const GZIP_DEFLATE: u8 = 0x08;

/// A failure reading or writing the format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The image is shorter than its header, or than its length field says.
    Short { need: usize, have: usize },
    /// Offset 0 is not `K230`.
    Magic,
    /// The type word is not 0 (only the unencrypted form is supported).
    Type(u32),
    /// Bytes 44..528 are not all zero.
    Padding,
    /// The payload's SHA-256 does not match the header.
    Digest,
    /// The gzip stream is too short to have a header and trailer.
    GzipShort,
    /// The gzip stream's CM byte is neither 0x08 nor the priv marker.
    GzipMethod(u8),
    /// The stream does not inflate to its ISIZE and CRC-32.
    GzipCheck,
    /// A uImage name longer than the 31 bytes its field holds.
    Name(usize),
    /// The data is longer than a uImage size field holds.
    TooLarge,
    /// Offset 0 of a uImage is not its magic.
    UImageMagic,
    /// The uImage header CRC does not match the header.
    UImageHeaderCrc,
    /// The uImage data CRC does not match the data.
    UImageDataCrc,
    /// The uImage name is not text.
    UImageName,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Short { need, have } => write!(f, "image is {have} bytes, needs {need}"),
            Error::Magic => write!(f, "offset 0 is not K230"),
            Error::Type(t) => write!(f, "type word is {t}, only 0 (no encryption) is supported"),
            Error::Padding => write!(f, "bytes 44..528 of the header are not all zero"),
            Error::Digest => write!(f, "the payload's SHA-256 does not match the header"),
            Error::GzipShort => write!(f, "gzip stream shorter than its header and trailer"),
            Error::GzipMethod(cm) => write!(f, "gzip CM byte is {cm:#04x}, not 0x08 or the priv marker"),
            Error::GzipCheck => write!(f, "gzip stream does not inflate to its ISIZE and CRC-32"),
            Error::Name(n) => write!(f, "uImage name is {n} bytes, the field holds 31"),
            Error::TooLarge => write!(f, "data is larger than a uImage size field holds"),
            Error::UImageMagic => write!(f, "uImage magic is wrong"),
            Error::UImageHeaderCrc => write!(f, "uImage header CRC does not match the header"),
            Error::UImageDataCrc => write!(f, "uImage data CRC does not match the data"),
            Error::UImageName => write!(f, "uImage name is not text"),
            Error::Name(n) => write!(f, "uImage name is {n} bytes, the field holds 31"),
            Error::TooLarge => write!(f, "data is larger than a uImage size field holds"),
            Error::UImageMagic => write!(f, "uImage magic is wrong"),
            Error::UImageHeaderCrc => write!(f, "uImage header CRC does not match the header"),
            Error::UImageDataCrc => write!(f, "uImage data CRC does not match the data"),
            Error::UImageName => write!(f, "uImage name is not text"),
        }
    }
}

impl std::error::Error for Error {}

/// A first-stage image read back: its header's fields and the payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firstage<'a> {
    /// The payload length from the header (the version word included).
    pub payload_len: u32,
    /// The SHA-256 the header carries.
    pub digest: [u8; 32],
    /// The payload: the version word, then the data.
    pub payload: &'a [u8],
}

/// Wraps `data` (an SPL, or a U-Boot uImage) as a first-stage image: the
/// zero version word, then the 528-byte header over that payload.
pub fn wrap_firstage(data: &[u8]) -> Vec<u8> {
    let mut payload = vec![0u8; VERSION_LEN];
    payload.extend_from_slice(data);
    let digest: [u8; 32] = Sha256::digest(&payload).into();

    let mut out = vec![0u8; HEADER_LEN];
    out[0..4].copy_from_slice(MAGIC);
    out[4..8].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    out[8..12].copy_from_slice(&0u32.to_le_bytes());
    out[12..44].copy_from_slice(&digest);
    out.extend_from_slice(&payload);
    out
}

/// Reads a first-stage image: checks the magic, the type, the zero padding,
/// the length against the image, and the payload's SHA-256.
pub fn parse_firstage(image: &[u8]) -> Result<Firstage<'_>, Error> {
    if image.len() < HEADER_LEN {
        return Err(Error::Short { need: HEADER_LEN, have: image.len() });
    }
    if &image[0..4] != MAGIC {
        return Err(Error::Magic);
    }
    let payload_len = u32::from_le_bytes([image[4], image[5], image[6], image[7]]);
    let ty = u32::from_le_bytes([image[8], image[9], image[10], image[11]]);
    if ty != 0 {
        return Err(Error::Type(ty));
    }
    if image[44..HEADER_LEN].iter().any(|&b| b != 0) {
        return Err(Error::Padding);
    }
    let end = HEADER_LEN + payload_len as usize;
    if image.len() < end {
        return Err(Error::Short { need: end, have: image.len() });
    }
    let payload = &image[HEADER_LEN..end];
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&image[12..44]);
    if Sha256::digest(payload).as_slice() != digest {
        return Err(Error::Digest);
    }
    Ok(Firstage { payload_len, digest, payload })
}

/// Makes an ordinary gzip stream of `raw` (header with no optional fields,
/// raw deflate at level 9, CRC-32 and ISIZE trailer), then sets its CM byte to
/// the priv marker, as the SDK's `k230_gzip` does with `sed`.
///
/// The deflate bytes are zlib's, not the SDK tool's: the SDK's stream does
/// not reproduce bit for bit from zlib at any level, and the SPL does not
/// need it to. What the SPL needs is a valid stream with a correct CRC-32 and
/// ISIZE, and the header's digest and the uImage's data CRC are computed over
/// whatever stream is written.
pub fn priv_gzip(raw: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = vec![0x1f, 0x8b, GZIP_DEFLATE, 0, 0, 0, 0, 0, 0, 0x03];
    let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(9));
    enc.write_all(raw).map_err(|_| Error::GzipCheck)?;
    let body = enc.finish().map_err(|_| Error::GzipCheck)?;
    out.extend_from_slice(&body);
    let mut crc = flate2::Crc::new();
    crc.update(raw);
    out.extend_from_slice(&crc.sum().to_le_bytes());
    out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    out[2] = PRIV_GZIP_CM;
    Ok(out)
}

/// What the SPL does to a priv-marked stream before inflating it: sets the
/// CM byte back to deflate, and checks that the stream inflates to its
/// ISIZE and CRC-32. Returns the inflated data.
pub fn undo_priv_gzip(stream: &[u8]) -> Result<Vec<u8>, Error> {
    if stream.len() < 18 {
        return Err(Error::GzipShort);
    }
    let cm = stream[2];
    if cm != GZIP_DEFLATE && cm != PRIV_GZIP_CM {
        return Err(Error::GzipMethod(cm));
    }
    let n = stream.len();
    let crc = u32::from_le_bytes([stream[n - 8], stream[n - 7], stream[n - 6], stream[n - 5]]);
    let isize = u32::from_le_bytes([stream[n - 4], stream[n - 3], stream[n - 2], stream[n - 1]]);
    let mut dec = flate2::read::DeflateDecoder::new(&stream[10..n - 8]);
    let mut raw = Vec::with_capacity(isize as usize);
    std::io::Read::read_to_end(&mut dec, &mut raw).map_err(|_| Error::GzipCheck)?;
    let mut sum = flate2::Crc::new();
    sum.update(&raw);
    if raw.len() as u32 != isize || sum.sum() != crc {
        return Err(Error::GzipCheck);
    }
    Ok(raw)
}

/// The legacy uImage header (U-Boot `include/image.h`, `legacy_img_hdr`): 64
/// bytes, every multi-byte field big-endian. Offsets: magic 0, header CRC 4,
/// time 8, size 12, load 16, entry 20, data CRC 24, OS 28, arch 29, type 30,
/// compression 31, name 32..64 (NUL-padded). The header CRC is the CRC-32 of
/// the header with its own CRC field zeroed; the data CRC is the CRC-32 of the
/// data.
pub const UIMAGE_MAGIC: u32 = 0x2705_1956;
/// The header's length.
pub const UIMAGE_HEADER_LEN: usize = 64;
/// Values from the probe image: its U-Boot uImage is OS 17, arch 26, type 5
/// (firmware), comp 1 (gzip); its OpenSBI one is OS 5, arch 26, type 2
/// (kernel), comp 0 (none). Both have load and entry 0 (the build's
/// CONFIG_SYS_TEXT_BASE is 0; OpenSBI is loaded by U-Boot's bootm).
pub const OS_LINUX: u8 = 5;
pub const OS_U_BOOT: u8 = 17;
pub const ARCH_RISCV: u8 = 26;
pub const TYPE_KERNEL: u8 = 2;
pub const TYPE_FIRMWARE: u8 = 5;
pub const COMP_NONE: u8 = 0;
pub const COMP_GZIP: u8 = 1;

/// The fields of a uImage header the caller chooses. `time` is the build time
/// in seconds: 0 makes the image reproducible. The probe image has 1794142272
/// for U-Boot and 1794142285 for OpenSBI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UImageFields<'a> {
    pub os: u8,
    pub arch: u8,
    pub kind: u8,
    pub comp: u8,
    pub load: u32,
    pub entry: u32,
    pub time: u32,
    pub name: &'a str,
}

fn crc32(parts: &[&[u8]]) -> u32 {
    let mut crc = flate2::Crc::new();
    for part in parts {
        crc.update(part);
    }
    crc.sum()
}

/// The 64-byte uImage header for data of `size` bytes whose CRC-32 is `dcrc`.
pub fn uimage_header(fields: &UImageFields<'_>, size: u32, dcrc: u32) -> Result<[u8; UIMAGE_HEADER_LEN], Error> {
    let name = fields.name.as_bytes();
    if name.len() > 31 {
        return Err(Error::Name(name.len()));
    }
    let mut h = [0u8; UIMAGE_HEADER_LEN];
    h[0..4].copy_from_slice(&UIMAGE_MAGIC.to_be_bytes());
    h[8..12].copy_from_slice(&fields.time.to_be_bytes());
    h[12..16].copy_from_slice(&size.to_be_bytes());
    h[16..20].copy_from_slice(&fields.load.to_be_bytes());
    h[20..24].copy_from_slice(&fields.entry.to_be_bytes());
    h[24..28].copy_from_slice(&dcrc.to_be_bytes());
    h[28] = fields.os;
    h[29] = fields.arch;
    h[30] = fields.kind;
    h[31] = fields.comp;
    h[32..32 + name.len()].copy_from_slice(name);
    let hcrc = crc32(&[&h[0..4], &[0u8; 4], &h[8..]]);
    h[4..8].copy_from_slice(&hcrc.to_be_bytes());
    Ok(h)
}

/// A uImage over `data`: its header, then the data.
pub fn uimage(fields: &UImageFields<'_>, data: &[u8]) -> Result<Vec<u8>, Error> {
    let size = u32::try_from(data.len()).map_err(|_| Error::TooLarge)?;
    let dcrc = crc32(&[data]);
    let mut out = uimage_header(fields, size, dcrc)?.to_vec();
    out.extend_from_slice(data);
    Ok(out)
}

/// The image the K230 ROM and SPL take as the second stage: the U-Boot
/// uImage over the priv gzip of `raw` (`uboot.bin`).
pub fn uboot_uimage(raw: &[u8], time: u32) -> Result<Vec<u8>, Error> {
    let stream = priv_gzip(raw)?;
    uimage(
        &UImageFields { os: OS_U_BOOT, arch: ARCH_RISCV, kind: TYPE_FIRMWARE, comp: COMP_GZIP, load: 0, entry: 0, time, name: "uboot" },
        &stream,
    )
}

/// The OpenSBI firmware the boot partition carries for U-Boot's `bootm`: a
/// kernel-type uImage over the raw `fw_jump.bin`, no compression.
pub fn opensbi_uimage(raw: &[u8], time: u32) -> Result<Vec<u8>, Error> {
    uimage(
        &UImageFields { os: OS_LINUX, arch: ARCH_RISCV, kind: TYPE_KERNEL, comp: COMP_NONE, load: 0, entry: 0, time, name: "linux" },
        raw,
    )
}

/// A uImage's header fields, read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UImageInfo {
    pub os: u8,
    pub arch: u8,
    pub kind: u8,
    pub comp: u8,
    pub load: u32,
    pub entry: u32,
    pub time: u32,
    pub size: u32,
    pub dcrc: u32,
    pub name: String,
}

/// Parses a uImage header's fields back out (big-endian), checking its magic,
/// its header CRC and, given the data, the data CRC.
pub fn check_uimage(image: &[u8]) -> Result<UImageInfo, Error> {
    if image.len() < UIMAGE_HEADER_LEN {
        return Err(Error::Short { need: UIMAGE_HEADER_LEN, have: image.len() });
    }
    let be = |o: usize| u32::from_be_bytes([image[o], image[o + 1], image[o + 2], image[o + 3]]);
    if be(0) != UIMAGE_MAGIC {
        return Err(Error::UImageMagic);
    }
    let hcrc = be(4);
    let mut h = [0u8; UIMAGE_HEADER_LEN];
    h.copy_from_slice(&image[..UIMAGE_HEADER_LEN]);
    h[4..8].fill(0);
    if crc32(&[&h]) != hcrc {
        return Err(Error::UImageHeaderCrc);
    }
    let size = be(12) as usize;
    if image.len() < UIMAGE_HEADER_LEN + size {
        return Err(Error::Short { need: UIMAGE_HEADER_LEN + size, have: image.len() });
    }
    if crc32(&[&image[UIMAGE_HEADER_LEN..UIMAGE_HEADER_LEN + size]]) != be(24) {
        return Err(Error::UImageDataCrc);
    }
    let name_end = image[32..64].iter().position(|&b| b == 0).unwrap_or(32);
    let name = std::str::from_utf8(&image[32..32 + name_end]).map_err(|_| Error::UImageName)?;
    Ok(UImageInfo {
        os: image[28],
        arch: image[29],
        kind: image[30],
        comp: image[31],
        load: be(16),
        entry: be(20),
        time: be(8),
        size: size as u32,
        dcrc: be(24),
        name: name.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe image's SPL header: the first 528 bytes of the blob at 1 MiB,
    /// whose payload is 209,236 bytes (not committed; see the PR notes).
    const SPL_HEADER: &[u8] = include_bytes!("../tests/fixtures/spl-1m.header.bin");
    /// The probe image's U-Boot header, at 2 MiB: payload 314,739 bytes.
    const UBOOT_HEADER: &[u8] = include_bytes!("../tests/fixtures/uboot-2m.header.bin");

    #[test]
    fn real_spl_header_fields() {
        // Only the header is here, so parse fails on the short payload, which
        // is the expected error; the fields are read from the header itself.
        assert_eq!(le32(&SPL_HEADER[4..8]), 209_236);
        assert_eq!(le32(&SPL_HEADER[8..12]), 0);
        assert_eq!(&SPL_HEADER[0..4], MAGIC);
        assert!(SPL_HEADER[44..HEADER_LEN].iter().all(|&b| b == 0));
        assert_eq!(
            hex(&SPL_HEADER[12..44]),
            "4999d7e5c39f3d40ad752fb34bea38f233f10b50b2f7eb45726332ed29f618bc"
        );
        assert!(matches!(parse_firstage(SPL_HEADER), Err(Error::Short { .. })));
    }

    #[test]
    fn real_uboot_header_fields() {
        assert_eq!(le32(&UBOOT_HEADER[4..8]), 314_739);
        assert_eq!(
            hex(&UBOOT_HEADER[12..44]),
            "3c3e7604093a2577f657b0a19eb64f06dcbee6c96285d89c17ce3031fd861e2f"
        );
        assert!(UBOOT_HEADER[44..HEADER_LEN].iter().all(|&b| b == 0));
    }

    #[test]
    fn wrap_then_parse_round_trips() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7 % 251) as u8).collect();
        let image = wrap_firstage(&data);
        assert_eq!(image.len(), HEADER_LEN + VERSION_LEN + data.len());
        assert_eq!(&image[0..4], MAGIC);
        let parsed = parse_firstage(&image).expect("parses");
        assert_eq!(parsed.payload_len as usize, VERSION_LEN + data.len());
        assert_eq!(&parsed.payload[..VERSION_LEN], &[0u8; 4]);
        assert_eq!(&parsed.payload[VERSION_LEN..], &data[..]);
    }

    #[test]
    fn wrap_matches_the_python_layout_bytes() {
        // The layout the Python writes: the digest is over the payload with
        // its version word, and bytes 44..528 are zero.
        let image = wrap_firstage(b"spl");
        let digest: [u8; 32] = Sha256::digest(&image[HEADER_LEN..]).into();
        assert_eq!(&image[12..44], &digest);
        assert!(image[44..HEADER_LEN].iter().all(|&b| b == 0));
    }

    #[test]
    fn parse_refuses_a_bad_digest_magic_type_or_padding() {
        let good = wrap_firstage(b"payload");
        let mut bad = good.clone();
        bad[HEADER_LEN] ^= 1;
        assert_eq!(parse_firstage(&bad), Err(Error::Digest));
        let mut bad = good.clone();
        bad[0] = b'X';
        assert_eq!(parse_firstage(&bad), Err(Error::Magic));
        let mut bad = good.clone();
        bad[8] = 1;
        assert_eq!(parse_firstage(&bad), Err(Error::Type(1)));
        let mut bad = good;
        bad[100] = 1;
        assert_eq!(parse_firstage(&bad), Err(Error::Padding));
    }

    #[test]
    fn priv_gzip_round_trips_and_is_marked() {
        let raw: Vec<u8> = (0..50_000u32).flat_map(|i| (i % 97).to_le_bytes()).collect();
        let stream = priv_gzip(&raw).expect("compresses");
        assert_eq!(&stream[0..2], &[0x1f, 0x8b]);
        assert_eq!(stream[2], PRIV_GZIP_CM);
        assert_eq!(undo_priv_gzip(&stream).expect("inflates"), raw);
    }

    #[test]
    fn undo_refuses_a_bad_trailer() {
        let raw = b"hello, second stage".to_vec();
        let mut stream = priv_gzip(&raw).expect("compresses");
        let n = stream.len();
        stream[n - 8] ^= 1; // CRC-32
        assert_eq!(undo_priv_gzip(&stream), Err(Error::GzipCheck));
        let mut stream = priv_gzip(&raw).expect("compresses");
        stream[2] = 0x07;
        assert_eq!(undo_priv_gzip(&stream), Err(Error::GzipMethod(7)));
    }

    fn le32(b: &[u8]) -> u32 {
        u32::from_le_bytes([b[0], b[1], b[2], b[3]])
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    const UBOOT_UIMAGE_HEADER: &[u8] = include_bytes!("../tests/fixtures/uboot-uimage.header.bin");
    const OPENSBI_UIMAGE_HEADER: &[u8] = include_bytes!("../tests/fixtures/opensbi-uimage.header.bin");

    #[test]
    fn uboot_uimage_header_reproduces_the_probe_bytes() {
        // The probe's U-Boot uImage: size 314,671, data CRC 0x97648f44, time
        // 1794142272. The header is rebuilt from those fields and must be the
        // 64 bytes the image holds (its header CRC is checked too).
        let fields = UImageFields {
            os: OS_U_BOOT, arch: ARCH_RISCV, kind: TYPE_FIRMWARE, comp: COMP_GZIP,
            load: 0, entry: 0, time: 1_794_142_272, name: "uboot",
        };
        let h = uimage_header(&fields, 314_671, 0x9764_8f44).expect("header");
        assert_eq!(&h[..], UBOOT_UIMAGE_HEADER);
        assert_eq!(le_or_be_hcrc(&h), 0xa7f1_4645);
    }

    #[test]
    fn opensbi_uimage_header_reproduces_the_probe_bytes() {
        // The probe's fw_jump_add_uboot_head.bin: size 139,656, data CRC
        // 0x91a98d33, time 1794142285, kernel type, no compression.
        let fields = UImageFields {
            os: OS_LINUX, arch: ARCH_RISCV, kind: TYPE_KERNEL, comp: COMP_NONE,
            load: 0, entry: 0, time: 1_794_142_285, name: "linux",
        };
        let h = uimage_header(&fields, 139_656, 0x91a9_8d33).expect("header");
        assert_eq!(&h[..], OPENSBI_UIMAGE_HEADER);
    }

    #[test]
    fn check_uimage_reads_back_what_was_written() {
        let data = b"raw second stage".to_vec();
        let fields = UImageFields {
            os: OS_LINUX, arch: ARCH_RISCV, kind: TYPE_KERNEL, comp: COMP_NONE,
            load: 0, entry: 0, time: 0, name: "linux",
        };
        let image = uimage(&fields, &data).expect("uimage");
        let info = check_uimage(&image).expect("checks");
        assert_eq!(info.size as usize, data.len());
        assert_eq!(info.name, "linux");
        assert_eq!(info.time, 0);
        let mut bad = image.clone();
        bad[HEADER_LEN_FOR_TEST] ^= 1;
        assert_eq!(check_uimage(&bad), Err(Error::UImageDataCrc));
        let mut bad = image;
        bad[9] ^= 1;
        assert_eq!(check_uimage(&bad), Err(Error::UImageHeaderCrc));
    }

    #[test]
    fn uboot_image_is_priv_gzip_inside_a_uimage_inside_the_first_stage() {
        let raw: Vec<u8> = (0..20_000u32).map(|i| (i % 211) as u8).collect();
        let uimage = uboot_uimage(&raw, 0).expect("uimage");
        let info = check_uimage(&uimage).expect("checks");
        assert_eq!((info.os, info.kind, info.comp), (OS_U_BOOT, TYPE_FIRMWARE, COMP_GZIP));
        // The data is the priv-marked stream, which undoes to the raw U-Boot.
        let stream = &uimage[UIMAGE_HEADER_LEN..];
        assert_eq!(stream[2], PRIV_GZIP_CM);
        assert_eq!(undo_priv_gzip(stream).expect("inflates"), raw);
        // The first stage over it passes its own check.
        let image = wrap_firstage(&uimage);
        assert_eq!(parse_firstage(&image).expect("parses").payload_len as usize, VERSION_LEN + uimage.len());
    }

    const HEADER_LEN_FOR_TEST: usize = UIMAGE_HEADER_LEN + 3;

    fn le_or_be_hcrc(h: &[u8; UIMAGE_HEADER_LEN]) -> u32 {
        u32::from_be_bytes([h[4], h[5], h[6], h[7]])
    }
}
