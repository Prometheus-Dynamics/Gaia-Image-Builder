//! The LZMA2 dictionary an xz stream declares, read from its first block
//! header, and the limit a board holds a first-flash stream to.
//!
//! A board refuses a stream whose dictionary is larger than 8 MiB, because
//! its decoder memory is fixed at dictionary plus 256 KiB on 64 MB-class
//! boards. The writer checks each stream it makes by reading the header back
//! from the bytes on disk; it does not trust the preset it asked for.

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// The largest dictionary a board takes: 8 MiB.
pub const LIMIT: u64 = 8 * 1024 * 1024;

const STREAM_HEADER_LEN: usize = 12;
const MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0x00];
/// The LZMA2 filter id in an xz block header.
const FILTER_LZMA2: u64 = 0x21;
/// The largest block header: a size byte of 255 gives (255 + 1) * 4 bytes.
const MAX_BLOCK_HEADER: usize = 1024;

/// The dictionary of the stream in `path`, read back from its first block
/// header (see [`dictionary_size`]).
pub fn read_back(path: &Path) -> Result<u64, String> {
    let file = File::open(path).map_err(|e| format!("read back: {e}"))?;
    let mut prefix = Vec::new();
    file.take((STREAM_HEADER_LEN + MAX_BLOCK_HEADER) as u64)
        .read_to_end(&mut prefix)
        .map_err(|e| format!("read back: {e}"))?;
    dictionary_size(&prefix)
}

/// The dictionary size, in bytes, that the first block of an xz stream
/// declares. The stream's bytes start at `stream[0]`. Refused: not an xz
/// stream, a stream with no block, a block with more than one filter, a
/// filter other than LZMA2, and an LZMA2 property above 40.
pub fn dictionary_size(stream: &[u8]) -> Result<u64, String> {
    if stream.get(..MAGIC.len()) != Some(&MAGIC[..]) {
        return Err("not an xz stream".into());
    }
    let size_byte = *stream
        .get(STREAM_HEADER_LEN)
        .ok_or("an xz stream with no block")?;
    if size_byte == 0 {
        return Err("an xz stream with no block".into());
    }
    let len = (usize::from(size_byte) + 1) * 4;
    let header = stream
        .get(STREAM_HEADER_LEN..STREAM_HEADER_LEN + len)
        .ok_or("a block header cut short")?;
    let flags = header[1];
    let filters = u64::from(flags & 0x03) + 1;
    if filters != 1 {
        return Err(format!(
            "a block with {filters} filters; only LZMA2 is allowed"
        ));
    }
    let mut pos = 2;
    if flags & 0x40 != 0 {
        varint(header, &mut pos)?;
    }
    if flags & 0x80 != 0 {
        varint(header, &mut pos)?;
    }
    let id = varint(header, &mut pos)?;
    let props_len = varint(header, &mut pos)?;
    if id != FILTER_LZMA2 {
        return Err(format!("filter {id:#x} is not LZMA2 (0x21)"));
    }
    if props_len != 1 {
        return Err("LZMA2 properties that are not one byte".into());
    }
    let property = *header.get(pos).ok_or("a block header cut short")?;
    lzma2_dictionary(property)
}

/// The dictionary an LZMA2 property byte names, as the xz file format gives
/// it: `(2 | (b & 1)) << (b / 2 + 11)` for `b` in 0..=39, and 4 GiB - 1 for 40.
pub fn lzma2_dictionary(property: u8) -> Result<u64, String> {
    match property {
        0..=39 => Ok((2 | u64::from(property & 1)) << (u64::from(property) / 2 + 11)),
        40 => Ok(u64::from(u32::MAX)),
        _ => Err(format!("LZMA2 dictionary property {property} is not valid")),
    }
}

/// An xz multibyte integer at `pos` in `bytes`, advancing `pos`.
fn varint(bytes: &[u8], pos: &mut usize) -> Result<u64, String> {
    let mut value = 0_u64;
    for shift in 0..9 {
        let byte = *bytes.get(*pos).ok_or("a block header cut short")?;
        *pos += 1;
        value |= u64::from(byte & 0x7f) << (7 * shift);
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("a block header with a multibyte integer too long".into())
}
