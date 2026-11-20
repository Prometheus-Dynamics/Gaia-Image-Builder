//! The image's xz stream, put together block by block.
//!
//! liblzma's multithreaded encoder makes each block of a fixed size on its
//! own: a block's bytes depend only on that block's input. A disk image is
//! mostly holes, and every block that is all zeros is the same input, so it
//! is the same bytes. The stream is therefore made here from its parts: each
//! block that holds data is encoded, the all-zero block is encoded once and
//! its bytes are written for every block of zeros, and the stream header,
//! index and footer are written as the xz file format gives them. The bytes
//! are those the multithreaded encoder writes for the whole image at the same
//! block size (the tests compare them), and any xz reader reads them.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

const STREAM_HEADER_LEN: usize = 12;
const STREAM_FOOTER_LEN: usize = 12;
const FOOTER_MAGIC: [u8; 2] = *b"YZ";

/// One block of a stream: its bytes (header, compressed data, padding and
/// check) and the two sizes the stream's index lists for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Block {
    pub bytes: Vec<u8>,
    pub unpadded: u64,
    pub uncompressed: u64,
}

/// How a stream was made: how many blocks, and how many of them were all
/// zeros and written from the one encoded zero block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Made {
    pub blocks: u64,
    pub zero_blocks: u64,
    /// CPU time the encoding used beside the calling thread, in
    /// nanoseconds: the process's CPU over the encode less the calling
    /// thread's. liblzma's encoder runs in threads of its own, which only
    /// the process's clock sees; other work inside the process at the same
    /// time is counted too.
    pub worker_cpu_ns: u64,
}

/// Memory the encoding takes besides its workers, in bytes: the zero block
/// to compare with and the writer (measured about 340 MB).
pub(crate) const MEMORY_BASE: u64 = 384 << 20;
/// Memory each encoder thread takes, in bytes: its 64 MiB read buffer and
/// liblzma's encoder at preset 6 with its block in and out (measured about
/// 224 MB a worker on 4, 8 and 16 workers).
pub(crate) const MEMORY_PER_WORKER: u64 = 256 << 20;

/// CPU time this process has used, in nanoseconds.
fn process_cpu_ns() -> u64 {
    let time = rustix::time::clock_gettime(rustix::time::ClockId::ProcessCPUTime);
    u64::try_from(time.tv_sec).unwrap_or(0) * 1_000_000_000
        + u64::try_from(time.tv_nsec).unwrap_or(0)
}

/// `data` (at most one block of input) as the one block the multithreaded
/// encoder makes of it, with the stream header it writes. The encoder is
/// given `data` alone, with one worker, and the block is cut out of the
/// stream it writes, through that stream's index.
pub(crate) fn block_of(
    data: &[u8],
    encoder: &dyn Fn() -> Result<xz2::stream::Stream, String>,
) -> Result<(Block, [u8; STREAM_HEADER_LEN]), String> {
    let mut writer = xz2::write::XzEncoder::new_stream(Vec::new(), encoder()?);
    writer.write_all(data).map_err(|e| format!("encode: {e}"))?;
    let stream = writer.finish().map_err(|e| format!("encode: {e}"))?;
    let bad = || "the encoder wrote a stream that is not one block".to_string();
    let len = stream.len();
    if len < STREAM_HEADER_LEN + STREAM_FOOTER_LEN || stream[len - 2..] != FOOTER_MAGIC {
        return Err(bad());
    }
    let foot = &stream[len - STREAM_FOOTER_LEN..];
    let backward = u32::from_le_bytes([foot[4], foot[5], foot[6], foot[7]]);
    let index_len = (usize::try_from(backward).map_err(|_| bad())? + 1) * 4;
    let index_at = (len - STREAM_FOOTER_LEN)
        .checked_sub(index_len)
        .ok_or_else(bad)?;
    let index = &stream[index_at..len - STREAM_FOOTER_LEN];
    let mut pos = 1;
    if index.first() != Some(&0) || varint(index, &mut pos)? != 1 {
        return Err(bad());
    }
    let unpadded = varint(index, &mut pos)?;
    let uncompressed = varint(index, &mut pos)?;
    let end = STREAM_HEADER_LEN as u64 + unpadded.div_ceil(4) * 4;
    if end != index_at as u64 || uncompressed != data.len() as u64 {
        return Err(bad());
    }
    let mut header = [0_u8; STREAM_HEADER_LEN];
    header.copy_from_slice(&stream[..STREAM_HEADER_LEN]);
    Ok((
        Block {
            bytes: stream[STREAM_HEADER_LEN..index_at].to_vec(),
            unpadded,
            uncompressed,
        },
        header,
    ))
}

/// The file at `input` as one xz stream in `out`, in blocks of `block`
/// bytes of input, each made by `encoder` (the multithreaded encoder at that
/// block size). Blocks that hold data are encoded on up to `threads`
/// threads; every full block of zeros is written from one encoded zero block.
pub(crate) fn write_stream(
    input: &Path,
    out: &Path,
    block: u64,
    threads: u32,
    encoder: &(dyn Fn() -> Result<xz2::stream::Stream, String> + Sync),
) -> Result<Made, String> {
    let io = |e: std::io::Error| format!("encode: {e}");
    let file = File::open(input).map_err(io)?;
    let len = file.metadata().map_err(io)?.len();
    let size = usize::try_from(block).map_err(|_| "a block size over memory".to_string())?;
    let count = len.div_ceil(block);
    let at = |i: u64| (i * block, block.min(len - i * block));

    // Which blocks are all zeros: only a full block can be the zero block.
    // A block the file system has no data in is a hole, all zeros, and is
    // not read; a block with data in it is read and compared, since data
    // can be zeros too.
    let zeros = vec![0_u8; size];
    let mut buffer = vec![0_u8; size];
    let mut zero = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
    let extents = data_extents(&file, len);
    for i in 0..count {
        let (offset, n) = at(i);
        let has_data = extents
            .as_ref()
            .is_none_or(|extents| extents.iter().any(|(s, e)| *s < offset + n && *e > offset));
        if n == block && !has_data {
            zero.push(true);
            continue;
        }
        let n = n as usize;
        file.read_exact_at(&mut buffer[..n], offset).map_err(io)?;
        zero.push(n == size && buffer == zeros);
    }
    drop(buffer);

    // Each block that holds data, encoded on its own, on up to `threads`.
    let data: Vec<u64> = (0..count).filter(|i| !zero[*i as usize]).collect();
    let made: Vec<Mutex<Option<Block>>> = data.iter().map(|_| Mutex::new(None)).collect();
    let header: Mutex<Option<[u8; STREAM_HEADER_LEN]>> = Mutex::new(None);
    let failed: Mutex<Option<String>> = Mutex::new(None);
    let next = AtomicUsize::new(0);
    let workers = (threads.max(1) as usize).min(data.len()).max(1);
    // The zero block, encoded once, beside the data blocks.
    let zero_block: Mutex<Option<Result<Block, String>>> = Mutex::new(None);
    let encoders = workers as u64 + u64::from(zero.iter().any(|z| *z));
    let _memory = gaia_store::hold_memory(MEMORY_BASE + MEMORY_PER_WORKER * encoders);
    let (process_start, thread_start) = (process_cpu_ns(), gaia_step::thread_cpu_ns());
    std::thread::scope(|scope| {
        if zero.iter().any(|z| *z) {
            scope.spawn(|| {
                let result = block_of(&zeros, encoder).map(|(block, head)| {
                    if let Ok(mut slot) = header.lock() {
                        slot.get_or_insert(head);
                    }
                    block
                });
                if let Ok(mut slot) = zero_block.lock() {
                    *slot = Some(result);
                }
            });
        }
        for _ in 0..workers {
            scope.spawn(|| {
                let mut buffer = vec![0_u8; size];
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    if k >= data.len() || failed.lock().map_or(true, |f| f.is_some()) {
                        return;
                    }
                    let (offset, n) = at(data[k]);
                    let input = &mut buffer[..n as usize];
                    let result = file
                        .read_exact_at(input, offset)
                        .map_err(io)
                        .and_then(|()| block_of(input, encoder));
                    match result {
                        Ok((block, head)) => {
                            if let Ok(mut slot) = made[k].lock() {
                                *slot = Some(block);
                            }
                            if let Ok(mut slot) = header.lock() {
                                slot.get_or_insert(head);
                            }
                        }
                        Err(why) => {
                            if let Ok(mut slot) = failed.lock() {
                                slot.get_or_insert(why);
                            }
                            return;
                        }
                    }
                }
            });
        }
    });
    let worker_cpu_ns = process_cpu_ns()
        .saturating_sub(process_start)
        .saturating_sub(gaia_step::thread_cpu_ns().saturating_sub(thread_start));
    if let Some(why) = failed
        .into_inner()
        .map_err(|_| "an encoder thread panicked")?
    {
        return Err(why);
    }
    let mut made: Vec<Option<Block>> = made
        .into_iter()
        .map(|slot| slot.into_inner().ok().flatten())
        .collect();
    let zero_block = zero_block
        .into_inner()
        .map_err(|_| "an encoder thread panicked")?
        .transpose()?;
    let header = header
        .into_inner()
        .map_err(|_| "an encoder thread panicked")?
        .ok_or("an image with no block")?;

    // Header, blocks in order, index, footer.
    let mut writer = BufWriter::with_capacity(1 << 20, File::create(out).map_err(io)?);
    writer.write_all(&header).map_err(io)?;
    let mut records = Vec::with_capacity(zero.len());
    let mut data_at = 0;
    for is_zero in &zero {
        let block = if *is_zero {
            zero_block.as_ref()
        } else {
            data_at += 1;
            made[data_at - 1].as_ref()
        }
        .ok_or("a block that was not encoded")?;
        writer.write_all(&block.bytes).map_err(io)?;
        records.push((block.unpadded, block.uncompressed));
    }
    made.clear();
    let index = index(&records);
    writer.write_all(&index).map_err(io)?;
    writer
        .write_all(&footer(&header, index.len()))
        .map_err(io)?;
    writer.flush().map_err(io)?;
    Ok(Made {
        blocks: count,
        zero_blocks: zero.iter().filter(|z| **z).count() as u64,
        worker_cpu_ns,
    })
}

/// The byte ranges of `file` (of `len` bytes) the file system holds data
/// for, from SEEK_DATA and SEEK_HOLE: everything else is a hole and reads as
/// zeros. `None` when the file system does not say, and every block is read.
fn data_extents(file: &File, len: u64) -> Option<Vec<(u64, u64)>> {
    use rustix::fs::{SeekFrom, seek};
    let mut extents = Vec::new();
    let mut at = 0;
    while at < len {
        let start = match seek(file, SeekFrom::Data(at)) {
            Ok(start) => start,
            // No data at or after `at`: the rest is a hole.
            Err(rustix::io::Errno::NXIO) => break,
            Err(_) => return None,
        };
        let end = seek(file, SeekFrom::Hole(start)).ok()?;
        if end <= start {
            return None;
        }
        extents.push((start, end.min(len)));
        at = end;
    }
    Some(extents)
}

/// A stream's index for blocks of these (unpadded, uncompressed) sizes: the
/// indicator, the count, the records, padding to four bytes and its CRC32.
pub(crate) fn index(records: &[(u64, u64)]) -> Vec<u8> {
    let mut index = vec![0_u8];
    put_varint(&mut index, records.len() as u64);
    for (unpadded, uncompressed) in records {
        put_varint(&mut index, *unpadded);
        put_varint(&mut index, *uncompressed);
    }
    while index.len() % 4 != 0 {
        index.push(0);
    }
    let crc = crc32(&index);
    index.extend_from_slice(&crc.to_le_bytes());
    index
}

/// The stream footer for an index of `index_len` bytes, with the stream
/// flags the header `header` has.
pub(crate) fn footer(
    header: &[u8; STREAM_HEADER_LEN],
    index_len: usize,
) -> [u8; STREAM_FOOTER_LEN] {
    let backward = u32::try_from(index_len / 4 - 1).unwrap_or(u32::MAX);
    let mut body = [0_u8; 6];
    body[..4].copy_from_slice(&backward.to_le_bytes());
    body[4..].copy_from_slice(&header[6..8]);
    let mut footer = [0_u8; STREAM_FOOTER_LEN];
    footer[..4].copy_from_slice(&crc32(&body).to_le_bytes());
    footer[4..10].copy_from_slice(&body);
    footer[10..].copy_from_slice(&FOOTER_MAGIC);
    footer
}

/// An xz multibyte integer.
fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// An xz multibyte integer at `pos` in `bytes`, advancing `pos`.
fn varint(bytes: &[u8], pos: &mut usize) -> Result<u64, String> {
    let mut value = 0_u64;
    for shift in 0..9 {
        let byte = *bytes.get(*pos).ok_or("an index cut short")?;
        *pos += 1;
        value |= u64::from(byte & 0x7f) << (7 * shift);
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("a multibyte integer too long".into())
}

/// CRC32 (IEEE, as the xz format uses for its index and footer).
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_is_the_ieee_one() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn multibyte_integers_round_trip() {
        for value in [0, 1, 0x7f, 0x80, 300, 64 << 20, u64::from(u32::MAX) + 7] {
            let mut bytes = Vec::new();
            put_varint(&mut bytes, value);
            let mut pos = 0;
            assert_eq!(varint(&bytes, &mut pos), Ok(value));
            assert_eq!(pos, bytes.len());
        }
    }
}
