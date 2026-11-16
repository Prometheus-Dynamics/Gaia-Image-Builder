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
}
