//! The container structure of the output, parsed here by hand (no `xz`
//! program, no decoder crate): the unsigned file is one stream; the signed
//! file is exactly that stream followed by a second, with no padding between
//! them. Each stream's header, block, index and footer are checked: the index
//! holds one record (the encoder is single-threaded, so one block), its CRC32
//! and the footer's CRC32 match, and the footer's flags equal the header's.

use gaia_flash_image::flash_image_with;
use gaia_step::Ctx;
use gaia_store::Store;
use gaia_types::{Image, ImageKind, Size};
use std::path::Path;

const MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0x00];
const CHECK_CRC64_FLAGS: [u8; 2] = [0x00, 0x04];
const HEADER_LEN: usize = 12;
const FOOTER_LEN: usize = 12;

/// The standard CRC32 (IEEE, reflected, poly 0xEDB88320), bitwise and
/// table-less.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// An xz multibyte integer at `pos`, advancing it.
fn varint(bytes: &[u8], pos: &mut usize) -> u64 {
    let mut value = 0_u64;
    for shift in 0..9 {
        let byte = bytes[*pos];
        *pos += 1;
        value |= u64::from(byte & 0x7f) << (7 * shift);
        if byte & 0x80 == 0 {
            return value;
        }
    }
    panic!("a multibyte integer too long");
}

/// One stream of a file, located from its footer. `start` and `end` are
/// byte offsets; `records` is the index's record count.
#[derive(Debug)]
struct Stream {
    start: usize,
    end: usize,
    records: u64,
    block_unpadded: u64,
}

/// Parses the stream that ends at `end` (its footer is the 12 bytes before
/// it), checking every structural rule. Panics name the failing rule.
fn parse_stream_ending_at(file: &[u8], end: usize) -> Stream {
    let footer = &file[end - FOOTER_LEN..end];
    let stored_crc = u32::from_le_bytes(footer[0..4].try_into().expect("4"));
    assert_eq!(&footer[10..12], b"YZ", "footer magic");
    assert_eq!(
        crc32(&footer[4..10]),
        stored_crc,
        "footer CRC32 covers the backward size and stream flags"
    );
    let footer_flags = [footer[8], footer[9]];
    assert_eq!(footer_flags, CHECK_CRC64_FLAGS, "footer flags: CRC64");
    let backward = u64::from(u32::from_le_bytes(footer[4..8].try_into().expect("4")));
    let index_len = ((backward + 1) * 4) as usize;
    let footer_start = end - FOOTER_LEN;
    let index_start = footer_start - index_len;
    let index = &file[index_start..footer_start];

    assert_eq!(index[0], 0x00, "index indicator");
    let mut pos = 1;
    let records = varint(index, &mut pos);
    let mut first_unpadded = 0;
    for record in 0..records {
        let unpadded = varint(index, &mut pos);
        let _uncompressed = varint(index, &mut pos);
        if record == 0 {
            first_unpadded = unpadded;
        }
    }
    assert_eq!(records, 1, "one record: the encoder is single-threaded");
    // Index padding up to the CRC32, which is the last four bytes.
    let crc_at = index.len() - 4;
    assert!(
        index[pos..crc_at].iter().all(|&b| b == 0),
        "index padding is zero"
    );
    assert_eq!(
        crc32(&index[..crc_at]),
        u32::from_le_bytes(index[crc_at..].try_into().expect("4")),
        "index CRC32"
    );

    // The block (padded to 4 bytes) sits right after the header, and the
    // index right after the block.
    let block_padded = first_unpadded.div_ceil(4) * 4;
    let start = index_start - block_padded as usize - HEADER_LEN;
    let header = &file[start..start + HEADER_LEN];
    assert_eq!(&header[..6], &MAGIC, "header magic");
    assert_eq!(&header[6..8], &CHECK_CRC64_FLAGS, "header flags: CRC64");
    assert_eq!(
        footer_flags,
        [header[6], header[7]],
        "footer flags equal the header's"
    );
    assert_eq!(
        start + HEADER_LEN + block_padded as usize,
        index_start,
        "one block, then the index, with no gap"
    );
    assert_eq!((end - start) % 4, 0, "stream length is a multiple of 4");
    Stream {
        start,
        end,
        records,
        block_unpadded: first_unpadded,
    }
}

/// Every stream in `file`, from the last back to the first.
fn streams(file: &[u8]) -> Vec<Stream> {
    let mut out = Vec::new();
    let mut end = file.len();
    while end > 0 {
        let stream = parse_stream_ending_at(file, end);
        end = stream.start;
        out.push(stream);
    }
    out.reverse();
    out
}

fn store_image(store_root: &Path, bytes: &[u8]) -> Image {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("disk.img");
    std::fs::write(&path, bytes).expect("write");
    let store = Store::open(store_root).expect("store");
    Image {
        kind: ImageKind::Disk,
        object: store.put_file(&path).expect("put").into(),
        size: Size(bytes.len() as u64),
        partitions: Vec::new(),
    }
}

fn output_bytes(root: &Path, signed: bool, raw: &[u8]) -> Vec<u8> {
    let store_dir = root.join(if signed { "signed" } else { "unsigned" });
    let store = store_dir.display().to_string();
    let image = store_image(&store_dir, raw);
    let text = gaia_sign::minisign::SecretKey::from_seed(&[7; 32], [1, 2, 3, 4, 5, 6, 7, 8])
        .to_file()
        .to_string();
    let env = move |name: &str| (name == "GAIA_FLASH_TEST_KEY").then(|| text.clone());
    let key = signed.then_some("GAIA_FLASH_TEST_KEY");
    let artifact = flash_image_with(&store, image, "flash-test", "disk", key, &env, &Ctx::new())
        .expect("flash");
    let path = artifact
        .object
        .path_in(&Store::open(&store_dir).expect("store"))
        .expect("in the store");
    std::fs::read(path).expect("read")
}

#[test]
fn signed_is_the_unsigned_stream_then_a_second_with_no_padding() {
    let root = tempfile::tempdir().expect("tempdir");
    let raw: Vec<u8> = (0..1_500_000_u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 21) as u8)
        .collect();
    let unsigned = output_bytes(root.path(), false, &raw);
    let signed = output_bytes(root.path(), true, &raw);

    assert!(
        signed.starts_with(&unsigned),
        "the unsigned file is a byte prefix of the signed one"
    );
    assert_eq!(
        &signed[unsigned.len()..unsigned.len() + 6],
        &MAGIC,
        "stream 2 header at the unsigned length"
    );
    assert_eq!(signed.len() % 4, 0, "file length is a multiple of 4");
    assert_eq!(unsigned.len() % 4, 0, "unsigned length is a multiple of 4");

    let one = streams(&unsigned);
    assert_eq!(one.len(), 1, "unsigned: exactly one stream");
    assert_eq!(one[0].records, 1, "unsigned: one block in the index");
    assert_eq!(one[0].end, unsigned.len());

    let two = streams(&signed);
    assert_eq!(two.len(), 2, "signed: exactly two streams");
    assert_eq!(
        two[0].end,
        unsigned.len(),
        "stream 1 ends where the unsigned file ends"
    );
    assert_eq!(
        two[1].start,
        unsigned.len(),
        "stream 2 starts right after it"
    );
    assert_eq!(two[0].records, 1, "stream 1: one record");
    assert_eq!(
        two[1].records, 1,
        "stream 2: one record (the count restarts per stream)"
    );
    assert!(two[1].block_unpadded > 0);
    assert!(
        two[1].end - two[1].start < 400,
        "the record's stream is small"
    );
}

#[test]
fn a_flipped_footer_crc_is_detected_by_the_parser() {
    let root = tempfile::tempdir().expect("tempdir");
    let raw: Vec<u8> = (0..70_000_u32).map(|i| (i % 251) as u8).collect();
    let mut unsigned = output_bytes(root.path(), false, &raw);
    let end = unsigned.len();
    unsigned[end - FOOTER_LEN + 5] ^= 1;
    let result = std::panic::catch_unwind(|| streams(&unsigned).len());
    assert!(
        result.is_err(),
        "a changed backward size is refused by the footer CRC"
    );
}
