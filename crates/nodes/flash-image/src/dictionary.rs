//! What an xz stream declares, read back from its bytes, and the limits a
//! board holds a first-flash stream to.
//!
//! A board refuses a block whose dictionary is larger than 8 MiB, because
//! its decoder memory is fixed at dictionary plus 256 KiB on 64 MB-class
//! boards, a block with a filter other than LZMA2, and a stream of more than
//! 8192 blocks. The writer checks each stream it makes by reading every
//! block's header back from the bytes on disk, found through the stream's
//! index; it does not trust the settings it asked for.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The largest dictionary a board takes: 8 MiB.
pub const LIMIT: u64 = 8 * 1024 * 1024;
/// The most blocks a board takes in the image's stream.
pub const MAX_BLOCKS: u64 = 8192;
/// The stream flags' check id for CRC64.
const CHECK_CRC64: u8 = 0x04;
const STREAM_FOOTER_LEN: u64 = 12;
const FOOTER_MAGIC: [u8; 2] = *b"YZ";

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
    block_dictionary(header)
}

/// The dictionary size, in bytes, that the block header `header` declares
/// (its bytes from the size byte on). Refused as [`dictionary_size`] says.
pub fn block_dictionary(header: &[u8]) -> Result<u64, String> {
    let flags = *header.get(1).ok_or("a block header cut short")?;
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

/// What the stream in a file declares, over all of its blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Declared {
    /// The blocks the index lists.
    pub blocks: u64,
    /// The largest dictionary any block declares.
    pub dictionary: u64,
    /// Whether every block's header states its compressed and uncompressed
    /// sizes, as the multithreaded encoder writes them and the one-thread
    /// encoder does not.
    pub sized: bool,
}

/// Every block of the one xz stream in `path`, read back: the block count
/// from the stream's index, and each block's header from the place the
/// index gives it. Refused: a file that is not one whole xz stream, a check
/// other than CRC64, an index that does not fit the file, and each block
/// [`block_dictionary`] refuses.
pub fn read_back_all(path: &Path) -> Result<Declared, String> {
    let io = |e: std::io::Error| format!("read back: {e}");
    let mut file = File::open(path).map_err(io)?;
    let len = file.metadata().map_err(io)?.len();
    if len < STREAM_HEADER_LEN as u64 + STREAM_FOOTER_LEN {
        return Err("not an xz stream".into());
    }
    let mut head = [0_u8; STREAM_HEADER_LEN];
    file.read_exact(&mut head).map_err(io)?;
    if head[..MAGIC.len()] != MAGIC {
        return Err("not an xz stream".into());
    }
    if head[7] != CHECK_CRC64 {
        return Err(format!(
            "the stream's check id is {:#x}, not CRC64 (0x4)",
            head[7]
        ));
    }
    let mut foot = [0_u8; STREAM_FOOTER_LEN as usize];
    file.seek(SeekFrom::Start(len - STREAM_FOOTER_LEN))
        .map_err(io)?;
    file.read_exact(&mut foot).map_err(io)?;
    if foot[10..] != FOOTER_MAGIC {
        return Err("the file does not end with an xz stream footer".into());
    }
    let index_len = (u64::from(u32::from_le_bytes([foot[4], foot[5], foot[6], foot[7]])) + 1) * 4;
    let index_at = (len - STREAM_FOOTER_LEN)
        .checked_sub(index_len)
        .filter(|at| *at >= STREAM_HEADER_LEN as u64)
        .ok_or("an xz index larger than the file")?;
    let mut index = vec![0_u8; index_len as usize];
    file.seek(SeekFrom::Start(index_at)).map_err(io)?;
    file.read_exact(&mut index).map_err(io)?;
    if index.first() != Some(&0) {
        return Err("an xz index that does not start with its indicator".into());
    }
    let mut pos = 1;
    let blocks = varint(&index, &mut pos)?;
    if blocks == 0 {
        return Err("an xz stream with no block".into());
    }
    let mut at = STREAM_HEADER_LEN as u64;
    let mut dictionary = 0;
    let mut sized = true;
    for _ in 0..blocks {
        let unpadded = varint(&index, &mut pos)?;
        let _uncompressed = varint(&index, &mut pos)?;
        if unpadded == 0 || at + unpadded > index_at {
            return Err("an xz index whose blocks do not fit the stream".into());
        }
        let mut size = [0_u8; 1];
        file.seek(SeekFrom::Start(at)).map_err(io)?;
        file.read_exact(&mut size).map_err(io)?;
        if size[0] == 0 {
            return Err("an xz index that points at no block".into());
        }
        let mut header = vec![0_u8; (usize::from(size[0]) + 1) * 4];
        header[0] = size[0];
        file.read_exact(&mut header[1..]).map_err(io)?;
        dictionary = dictionary.max(block_dictionary(&header)?);
        sized &= header[1] & 0xC0 == 0xC0;
        at += unpadded.div_ceil(4) * 4;
    }
    if at != index_at {
        return Err("an xz index that does not account for the whole stream".into());
    }
    Ok(Declared {
        blocks,
        dictionary,
        sized,
    })
}
