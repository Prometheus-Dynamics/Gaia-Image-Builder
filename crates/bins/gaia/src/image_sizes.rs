//! `gaia sizes --image <file>`: a disk image's sizes, read from the image.
//!
//! For an image this builder did not make (another builder's, or a
//! release's), so a comparison's two columns are measured by one tool. The
//! image may be xz-compressed. Read: its size compressed and raw, its
//! partitions (MBR or GPT) and what each holds by its first blocks, and
//! every file of a FAT partition, with the unpacked size of one that is
//! gzip (a kernel a firmware loads compressed).
//!
//! What is inside an ext4 partition is not read, so a root filesystem kept
//! as a file there, and the modules in it, are rows this tool states it did
//! not measure, not rows left out.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// One measured thing: what, bytes, and a detail.
pub type Row = (String, u64, String);

const SECTOR: u64 = 512;

fn at(file: &mut File, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
    let mut bytes = vec![0u8; len];
    file.seek(SeekFrom::Start(offset))?;
    let mut read = 0;
    while read < len {
        match file.read(&mut bytes[read..])? {
            0 => break,
            n => read += n,
        }
    }
    bytes.truncate(read);
    Ok(bytes)
}

fn u16le(b: &[u8], i: usize) -> u64 {
    u64::from(u16::from_le_bytes([b[i], b[i + 1]]))
}
fn u32le(b: &[u8], i: usize) -> u64 {
    u64::from(u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]))
}
fn u64le(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes([
        b[i],
        b[i + 1],
        b[i + 2],
        b[i + 3],
        b[i + 4],
        b[i + 5],
        b[i + 6],
        b[i + 7],
    ])
}

/// A writer that keeps only how much was written.
struct Counter(u64);

impl Write for Counter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The partitions of the image: byte offset, byte length and a name for
/// each (its number, and its GPT name when it has one).
fn partitions(file: &mut File) -> std::io::Result<Vec<(u64, u64, String)>> {
    let mbr = at(file, 0, 512)?;
    if mbr.len() < 512 || mbr[510] != 0x55 || mbr[511] != 0xAA {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    let protective = (0..4).any(|n| mbr[446 + n * 16 + 4] == 0xEE);
    if protective {
        let header = at(file, SECTOR, 92)?;
        if header.len() == 92 && &header[..8] == b"EFI PART" {
            let (first, count, size) = (u64le(&header, 72), u32le(&header, 80), u32le(&header, 84));
            for n in 0..count.min(128) {
                let entry = at(file, first * SECTOR + n * size, size as usize)?;
                if entry.len() < 128 || entry[..16].iter().all(|b| *b == 0) {
                    continue;
                }
                let (start, end) = (u64le(&entry, 32), u64le(&entry, 40));
                let name: String = entry[56..128]
                    .chunks(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .take_while(|c| *c != 0)
                    .filter_map(|c| char::from_u32(u32::from(c)))
                    .collect();
                let label = if name.is_empty() {
                    format!("partition {}", n + 1)
                } else {
                    format!("partition {} ({name})", n + 1)
                };
                found.push((start * SECTOR, (end + 1 - start) * SECTOR, label));
            }
            return Ok(found);
        }
    }
    for n in 0..4 {
        let entry = &mbr[446 + n * 16..446 + (n + 1) * 16];
        let (kind, start, sectors) = (entry[4], u32le(entry, 8), u32le(entry, 12));
        if kind != 0 && sectors != 0 {
            found.push((
                start * SECTOR,
                sectors * SECTOR,
                format!("partition {} (type {kind:#04x})", n + 1),
            ));
        }
    }
    Ok(found)
}

/// What a partition holds, by its first blocks.
fn holds(file: &mut File, offset: u64) -> std::io::Result<&'static str> {
    let head = at(file, offset, 2048)?;
    if head.len() < 2048 {
        return Ok("unknown");
    }
    Ok(if u32le(&head, 1024) == 0xE0F5_E1E2 {
        "erofs"
    } else if u16le(&head, 1080) == 0xEF53 {
        "ext2/3/4"
    } else if &head[..4] == b"hsqs" {
        "squashfs"
    } else if head[510] == 0x55
        && head[511] == 0xAA
        && (&head[54..57] == b"FAT" || &head[82..85] == b"FAT")
    {
        "fat"
    } else {
        "unknown"
    })
}

/// A FAT volume at `offset`: its geometry, read once.
struct Fat {
    offset: u64,
    cluster_bytes: u64,
    fat_at: u64,
    data_at: u64,
    /// 12, 16 or 32.
    bits: u8,
    root_at: u64,
    root_bytes: u64,
    root_cluster: u64,
}

impl Fat {
    fn open(file: &mut File, offset: u64) -> std::io::Result<Self> {
        let b = at(file, offset, 512)?;
        let bps = u16le(&b, 11).max(1);
        let spc = u64::from(b[13]).max(1);
        let reserved = u16le(&b, 14);
        let fats = u64::from(b[16]);
        let root_entries = u16le(&b, 17);
        let total = match u16le(&b, 19) {
            0 => u32le(&b, 32),
            n => n,
        };
        let fat_sectors = match u16le(&b, 22) {
            0 => u32le(&b, 36),
            n => n,
        };
        let root_sectors = (root_entries * 32).div_ceil(bps);
        let first_data = reserved + fats * fat_sectors + root_sectors;
        let clusters = total.saturating_sub(first_data) / spc;
        let bits = if clusters < 4085 {
            12
        } else if clusters < 65525 {
            16
        } else {
            32
        };
        Ok(Self {
            offset,
            cluster_bytes: spc * bps,
            fat_at: offset + reserved * bps,
            data_at: offset + first_data * bps,
            bits,
            root_at: offset + (reserved + fats * fat_sectors) * bps,
            root_bytes: root_sectors * bps,
            root_cluster: u32le(&b, 44),
        })
    }

    fn next(&self, file: &mut File, cluster: u64) -> std::io::Result<Option<u64>> {
        let (value, end) = match self.bits {
            12 => {
                let b = at(file, self.fat_at + cluster * 3 / 2, 2)?;
                let pair = u16le(&b, 0);
                let value = if cluster.is_multiple_of(2) {
                    pair & 0xFFF
                } else {
                    pair >> 4
                };
                (value, 0xFF8)
            }
            16 => (u16le(&at(file, self.fat_at + cluster * 2, 2)?, 0), 0xFFF8),
            _ => (
                u32le(&at(file, self.fat_at + cluster * 4, 4)?, 0) & 0x0FFF_FFFF,
                0x0FFF_FFF8,
            ),
        };
        Ok((value >= 2 && value < end).then_some(value))
    }

    /// The bytes of the chain from `first`, at most `limit` of them.
    fn chain(&self, file: &mut File, first: u64, limit: u64) -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        let mut cluster = Some(first);
        let mut hops = 0u32;
        while let Some(at_cluster) = cluster {
            if at_cluster < 2 || bytes.len() as u64 >= limit || hops > 4_000_000 {
                break;
            }
            let from = self.data_at + (at_cluster - 2) * self.cluster_bytes;
            bytes.extend(at(file, from, self.cluster_bytes as usize)?);
            cluster = self.next(file, at_cluster)?;
            hops += 1;
        }
        bytes.truncate(limit as usize);
        Ok(bytes)
    }

    /// Every file under the directory whose entries are `raw`, as its path,
    /// first cluster and size.
    fn walk(
        &self,
        file: &mut File,
        raw: &[u8],
        under: &str,
        depth: u32,
        into: &mut Vec<(String, u64, u64)>,
    ) -> std::io::Result<()> {
        let mut long: Vec<(u8, String)> = Vec::new();
        for entry in raw.as_chunks::<32>().0 {
            if entry[0] == 0 {
                break;
            }
            if entry[0] == 0xE5 {
                long.clear();
                continue;
            }
            let attr = entry[11];
            if attr == 0x0F {
                let units: Vec<u16> = [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30]
                    .iter()
                    .map(|i| u16::from_le_bytes([entry[*i], entry[*i + 1]]))
                    .take_while(|c| *c != 0 && *c != 0xFFFF)
                    .collect();
                long.push((entry[0] & 0x1F, String::from_utf16_lossy(&units)));
                continue;
            }
            let name = if long.is_empty() {
                let part = |bytes: &[u8], lower: bool| {
                    let text = String::from_utf8_lossy(bytes).trim_end().to_string();
                    if lower { text.to_lowercase() } else { text }
                };
                let (base, ext) = (
                    part(&entry[..8], entry[12] & 0x08 != 0),
                    part(&entry[8..11], entry[12] & 0x10 != 0),
                );
                if ext.is_empty() {
                    base
                } else {
                    format!("{base}.{ext}")
                }
            } else {
                long.sort_by_key(|(order, _)| *order);
                long.drain(..).map(|(_, text)| text).collect()
            };
            long.clear();
            if attr & 0x08 != 0 || name == "." || name == ".." {
                continue;
            }
            let first = (u16le(entry, 20) << 16) | u16le(entry, 26);
            let path = if under.is_empty() {
                name
            } else {
                format!("{under}/{name}")
            };
            if attr & 0x10 != 0 {
                if depth < 16 {
                    let inner = self.chain(file, first, 4 << 20)?;
                    self.walk(file, &inner, &path, depth + 1, into)?;
                }
            } else {
                into.push((path, first, u32le(entry, 28)));
            }
        }
        Ok(())
    }

    fn files(&self, file: &mut File) -> std::io::Result<Vec<(String, u64, u64)>> {
        let root = if self.bits == 32 {
            self.chain(file, self.root_cluster, 4 << 20)?
        } else {
            at(file, self.root_at, self.root_bytes as usize)?
        };
        let mut found = Vec::new();
        self.walk(file, &root, "", 0, &mut found)?;
        found.sort();
        let _ = self.offset;
        Ok(found)
    }
}

/// The rows of the image at `path`. A compressed image is unpacked to a
/// file under `scratch`, which is removed again.
pub fn measure(path: &Path, scratch: &Path) -> std::io::Result<Vec<Row>> {
    let mut rows: Vec<Row> = Vec::new();
    let stored = std::fs::metadata(path)?.len();
    let mut head = [0u8; 6];
    let read = File::open(path)?.read(&mut head)?;
    let xz = read == 6 && head == [0xFD, b'7', b'z', b'X', b'Z', 0];
    let unpacked = scratch.join(format!("gaia-sizes-{}.img", std::process::id()));
    let raw_path = if xz {
        let mut decoder = xz2::read::XzDecoder::new(File::open(path)?);
        std::io::copy(&mut decoder, &mut File::create(&unpacked)?)?;
        rows.push(("image xz".into(), stored, String::new()));
        unpacked.as_path()
    } else {
        path
    };
    let result = (|| -> std::io::Result<()> {
        let mut file = File::open(raw_path)?;
        let raw = file.metadata()?.len();
        rows.push(("image raw".into(), raw, String::new()));
        let mut unread: Vec<String> = Vec::new();
        // An image that is one filesystem and no disk: a partition's own
        // image. (Its first sector ends as a partition table's does.)
        let parts = if holds(&mut file, 0)? == "unknown" {
            partitions(&mut file)?
        } else {
            vec![(0, raw, "volume".to_string())]
        };
        for (offset, bytes, label) in parts {
            let kind = holds(&mut file, offset)?;
            rows.push((label.clone(), bytes, kind.into()));
            if kind != "fat" {
                if kind != "unknown" {
                    unread.push(format!("{label}, {kind}"));
                }
                continue;
            }
            let fat = Fat::open(&mut file, offset)?;
            let listed = fat.files(&mut file)?;
            let total: u64 = listed.iter().map(|(_, _, size)| size).sum();
            rows.push((
                format!("  {label}: files"),
                total,
                format!("{} files", listed.len()),
            ));
            for (name, first, size) in listed {
                rows.push((format!("  {name}"), size, String::new()));
                // A gzip file: what it unpacks to, a kernel's raw size.
                if size > 18 && fat.chain(&mut file, first, 2)? == [0x1F, 0x8B] {
                    let packed = fat.chain(&mut file, first, size)?;
                    let mut counter = Counter(0);
                    if std::io::copy(&mut flate2::read::GzDecoder::new(&packed[..]), &mut counter)
                        .is_ok()
                    {
                        rows.push((format!("  {name}"), counter.0, "unpacked (gzip)".into()));
                    }
                }
            }
        }
        for what in unread {
            rows.push((
                "not measured".into(),
                0,
                format!("what is inside {what}: this tool does not read it"),
            ));
        }
        Ok(())
    })();
    if xz {
        let _ = std::fs::remove_file(&unpacked);
    }
    result.map(|()| rows)
}
