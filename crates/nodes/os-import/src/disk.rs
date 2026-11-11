//! Partition tables (GPT, MBR) read from the image file, and what each
//! partition holds, told by its filesystem's magic.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::FileExt;

/// What a partition (or a bare filesystem image) holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fs {
    Ext,
    Fat,
    Other,
}

/// One partition: its 1-based number, name (GPT) and byte range.
#[derive(Clone, Debug)]
pub(crate) struct Part {
    pub number: u32,
    pub name: String,
    pub start: u64,
    pub len: u64,
    pub fs: Fs,
}

impl Part {
    pub(crate) fn describe(&self) -> String {
        let name = if self.name.is_empty() {
            String::new()
        } else {
            format!(" {:?}", self.name)
        };
        format!(
            "partition {}{name} ({:?}, {} bytes)",
            self.number, self.fs, self.len
        )
    }
}

/// The partitions of `file`: GPT, else MBR, else the whole file as one
/// filesystem.
pub(crate) fn partitions(file: &File) -> io::Result<Vec<Part>> {
    let size = file.metadata()?.len();
    let mut parts = gpt(file, size)?.map_or_else(|| mbr(file, size), Ok)?;
    if parts.is_empty() {
        parts.push(Part {
            number: 1,
            name: String::new(),
            start: 0,
            len: size,
            fs: Fs::Other,
        });
    }
    for part in &mut parts {
        part.fs = detect(file, part.start)?;
    }
    Ok(parts)
}

fn read_at(file: &File, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0; len];
    match file.read_exact_at(&mut buf, offset) {
        Ok(()) => Ok(buf),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("8 bytes"))
}

fn gpt(file: &File, size: u64) -> io::Result<Option<Vec<Part>>> {
    for sector in [512u64, 4096] {
        let header = read_at(file, sector, 92)?;
        if header.len() < 92 || &header[..8] != b"EFI PART" {
            continue;
        }
        let entries_lba = u64_at(&header, 72);
        let count = u32_at(&header, 80).min(1024);
        let entry_size = u32_at(&header, 84) as usize;
        if entry_size < 128 {
            return Err(io::Error::other("a GPT with entries under 128 bytes"));
        }
        let table = read_at(file, entries_lba * sector, count as usize * entry_size)?;
        let mut parts = Vec::new();
        for i in 0..count as usize {
            let Some(entry) = table.get(i * entry_size..(i + 1) * entry_size) else {
                break;
            };
            if entry[..16].iter().all(|b| *b == 0) {
                continue;
            }
            let (first, last) = (u64_at(entry, 32), u64_at(entry, 40));
            let name: Vec<u16> = entry[56..128]
                .chunks(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|c| *c != 0)
                .collect();
            let (start, end) = (first * sector, (last + 1) * sector);
            if last < first || end > size {
                return Err(io::Error::other(format!(
                    "GPT partition {} lies outside the image",
                    i + 1
                )));
            }
            parts.push(Part {
                number: i as u32 + 1,
                name: String::from_utf16_lossy(&name),
                start,
                len: end - start,
                fs: Fs::Other,
            });
        }
        return Ok(Some(parts));
    }
    Ok(None)
}

fn mbr(file: &File, size: u64) -> io::Result<Vec<Part>> {
    let sector = read_at(file, 0, 512)?;
    if sector.len() < 512 || sector[510..512] != [0x55, 0xaa] || detect(file, 0)? != Fs::Other {
        return Ok(Vec::new());
    }
    let mut parts = Vec::new();
    for i in 0..4 {
        let entry = &sector[446 + i * 16..446 + (i + 1) * 16];
        let kind = entry[4];
        // Empty, extended (its logical partitions are not read), protective.
        if matches!(kind, 0x00 | 0x05 | 0x0f | 0x85 | 0xee) {
            continue;
        }
        let start = u64::from(u32_at(entry, 8)) * 512;
        let len = u64::from(u32_at(entry, 12)) * 512;
        if start + len > size {
            return Err(io::Error::other(format!(
                "MBR partition {} lies outside the image",
                i + 1
            )));
        }
        parts.push(Part {
            number: i as u32 + 1,
            name: String::new(),
            start,
            len,
            fs: Fs::Other,
        });
    }
    Ok(parts)
}

/// The filesystem starting at `start`, by its magic.
fn detect(file: &File, start: u64) -> io::Result<Fs> {
    let head = read_at(file, start, 2048)?;
    if head.len() < 2048 {
        return Ok(Fs::Other);
    }
    if head[1080..1082] == [0x53, 0xef] {
        return Ok(Fs::Ext);
    }
    if head[510..512] == [0x55, 0xaa] && (&head[54..57] == b"FAT" || &head[82..85] == b"FAT") {
        return Ok(Fs::Fat);
    }
    Ok(Fs::Other)
}

/// A partition as its own seekable device. Writes are dropped: nothing
/// changes the stored image.
pub(crate) struct Slice {
    file: File,
    start: u64,
    len: u64,
    pos: u64,
}

impl Slice {
    pub(crate) fn new(file: File, part: &Part) -> Self {
        Self {
            file,
            start: part.start,
            len: part.len,
            pos: 0,
        }
    }
}

impl Read for Slice {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.len.saturating_sub(self.pos);
        let n = (buf.len() as u64).min(left) as usize;
        if n == 0 {
            return Ok(0);
        }
        let got = self.file.read_at(&mut buf[..n], self.start + self.pos)?;
        self.pos += got as u64;
        Ok(got)
    }
}

impl Write for Slice {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pos += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for Slice {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = pos.ok_or_else(|| io::Error::other("seek before the partition"))?;
        Ok(self.pos)
    }
}

impl ext4_view::Ext4Read for Slice {
    fn read(
        &mut self,
        start_byte: u64,
        dst: &mut [u8],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
        if start_byte + dst.len() as u64 > self.len {
            return Err("a read past the end of the partition".into());
        }
        self.file.read_exact_at(dst, self.start + start_byte)?;
        Ok(())
    }
}
