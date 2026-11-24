//! The dictionary-size parser: the LZMA2 property byte maps to the size the
//! xz file format gives, and a block with another filter is refused.

use gaia_flash_image::dictionary::{dictionary_size, lzma2_dictionary};

const MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0x00];

/// A stream: its 12-byte header, then `block` (the first block's header),
/// then zeros standing for the data and index. The parser reads only the
/// header, so the zeros are enough.
fn stream(block: &[u8]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&[0x00, 0x04, 0, 0, 0, 0]);
    out.extend_from_slice(block);
    out.extend_from_slice(&[0; 64]);
    out
}

/// A one-filter LZMA2 block header with property byte `property`: twelve
/// bytes, a size byte of 2 (`(2 + 1) * 4`), no sizes in the flags.
fn lzma2_block(property: u8) -> Vec<u8> {
    vec![0x02, 0x00, 0x21, 0x01, property, 0, 0, 0, 0, 0, 0, 0]
}

#[test]
fn property_0_is_4_kib() {
    assert_eq!(dictionary_size(&stream(&lzma2_block(0))), Ok(4096));
}

#[test]
fn property_1_is_6_kib() {
    assert_eq!(dictionary_size(&stream(&lzma2_block(1))), Ok(6144));
}

#[test]
fn property_22_is_8_mib_the_ceiling() {
    assert_eq!(
        dictionary_size(&stream(&lzma2_block(22))),
        Ok(8 * 1024 * 1024)
    );
}

#[test]
fn property_23_is_12_mib_and_over_the_ceiling() {
    assert_eq!(
        dictionary_size(&stream(&lzma2_block(23))),
        Ok(12 * 1024 * 1024)
    );
}

#[test]
fn property_40_is_4_gib_minus_one() {
    assert_eq!(
        dictionary_size(&stream(&lzma2_block(40))),
        Ok(u64::from(u32::MAX))
    );
}

#[test]
fn a_property_above_40_is_refused() {
    assert!(lzma2_dictionary(41).is_err());
    assert!(dictionary_size(&stream(&lzma2_block(41))).is_err());
}

#[test]
fn a_block_with_a_second_filter_is_refused() {
    // Size byte 3 (16 bytes), flags 0x01 (two filters): LZMA2 then delta.
    let block = [
        0x03, 0x01, 0x21, 0x01, 22, 0x03, 0x01, 0x00, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    let error = dictionary_size(&stream(&block)).expect_err("two filters");
    assert!(error.contains("2 filters"), "{error}");
}

#[test]
fn a_block_with_a_filter_other_than_lzma2_is_refused() {
    // One filter, x86 BCJ (0x04), no properties.
    let block = [0x02, 0x00, 0x04, 0x00, 0, 0, 0, 0, 0, 0, 0, 0];
    let error = dictionary_size(&stream(&block)).expect_err("BCJ");
    assert!(error.contains("not LZMA2"), "{error}");
}

#[test]
fn a_stream_that_is_not_xz_is_refused() {
    assert!(dictionary_size(b"not an xz stream at all, just text").is_err());
}

#[test]
fn a_stream_with_no_block_is_refused() {
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(&[0x00, 0x04, 0, 0, 0, 0, 0x00]);
    assert!(dictionary_size(&bytes).is_err());
}
