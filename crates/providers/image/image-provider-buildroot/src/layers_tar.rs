//! The tar side of image layers: a reader for Buildroot's `rootfs.tar` that
//! keeps each entry's owner, mode and where its data sits, and a writer that
//! emits a deterministic ustar stream from those entries.
//!
//! The writer keeps the source's uid, gid and mode (the rootfs ownership
//! Buildroot applied under fakeroot), and uses one fixed mtime for every
//! entry. Names that ustar cannot hold go in PAX records. The reader accepts
//! ustar, GNU long names and PAX headers, which is what `tar` writes.

use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

const BLOCK: usize = 512;
const NAME_MAX: usize = 100;
const PREFIX_MAX: usize = 155;
/// Octal fields hold at most this (11 digits for the size, 7 for the rest).
const SIZE_MAX: u64 = 0o77_777_777_777;
const ID_MAX: u64 = 0o7_777_777;

/// What a tar entry is, as far as a rootfs layer needs to know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EntryKind {
    Dir,
    File,
    Symlink,
    Hardlink,
    CharDev,
    BlockDev,
    Fifo,
}

/// One rootfs entry, read from the source tar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    /// Rootfs path without a leading or trailing slash; `""` is the root.
    pub(crate) path: String,
    pub(crate) kind: EntryKind,
    pub(crate) mode: u32,
    pub(crate) uid: u64,
    pub(crate) gid: u64,
    pub(crate) size: u64,
    /// Where a regular file's data starts in the source tar.
    pub(crate) offset: u64,
    /// Symlink target, or the path a hardlink points at (normalized).
    pub(crate) link: String,
    pub(crate) dev_major: u32,
    pub(crate) dev_minor: u32,
    /// sha256 (hex) of a regular file's data.
    pub(crate) digest: String,
}

impl Entry {
    /// A directory the layer needs above its members, when the source has
    /// no entry for it: root-owned, 0755, as Buildroot makes them.
    pub(crate) fn synthetic_dir(path: &str) -> Self {
        Self {
            path: path.to_string(),
            kind: EntryKind::Dir,
            mode: 0o755,
            uid: 0,
            gid: 0,
            size: 0,
            offset: 0,
            link: String::new(),
            dev_major: 0,
            dev_minor: 0,
            digest: String::new(),
        }
    }
}

/// Strips `./`, leading and trailing slashes: `./usr/bin/` is `usr/bin`.
pub(crate) fn normalize_path(name: &str) -> String {
    let mut path = name;
    while let Some(rest) = path.strip_prefix("./") {
        path = rest;
    }
    if path == "." {
        return String::new();
    }
    path.trim_matches('/').to_string()
}

/// Reads every entry of `tar`. A path that appears twice keeps its last
/// entry, as tar extraction does. Data is hashed while it is read.
pub(crate) fn read_entries(
    tar: &Path,
) -> Result<std::collections::BTreeMap<String, Entry>, String> {
    let file =
        File::open(tar).map_err(|error| format!("failed to open '{}': {error}", tar.display()))?;
    let mut reader = BufReader::new(file);
    let mut position = 0u64;
    let mut entries = std::collections::BTreeMap::new();
    let mut pax_path: Option<String> = None;
    let mut pax_linkpath: Option<String> = None;
    let mut pax_size: Option<u64> = None;
    let mut pax_uid: Option<u64> = None;
    let mut pax_gid: Option<u64> = None;
    let mut gnu_name: Option<String> = None;
    let mut gnu_link: Option<String> = None;
    loop {
        let mut header = [0u8; BLOCK];
        if !read_full(&mut reader, &mut header)? {
            break;
        }
        position += BLOCK as u64;
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        verify_checksum(&header, tar)?;
        let size = match pax_size.take() {
            Some(size) => size,
            None => parse_number(&header[124..136])?,
        };
        let typeflag = header[156];
        let data_blocks = size.div_ceil(BLOCK as u64) * BLOCK as u64;
        match typeflag {
            b'x' | b'g' | b'L' | b'K' => {
                let mut body = vec![0u8; size as usize];
                reader
                    .read_exact(&mut body)
                    .map_err(|error| format!("truncated tar in '{}': {error}", tar.display()))?;
                skip_padding(&mut reader, size)?;
                position += data_blocks;
                match typeflag {
                    b'L' => gnu_name = Some(c_string(&body)),
                    b'K' => gnu_link = Some(c_string(&body)),
                    b'x' => {
                        for (key, value) in pax_records(&body)? {
                            match key.as_str() {
                                "path" => pax_path = Some(value),
                                "linkpath" => pax_linkpath = Some(value),
                                "size" => pax_size = value.parse().ok(),
                                "uid" => pax_uid = value.parse().ok(),
                                "gid" => pax_gid = value.parse().ok(),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
                continue;
            }
            _ => {}
        }
        let name = match (pax_path.take(), gnu_name.take()) {
            (Some(name), _) | (None, Some(name)) => name,
            (None, None) => header_name(&header),
        };
        let link_name = match (pax_linkpath.take(), gnu_link.take()) {
            (Some(link), _) | (None, Some(link)) => link,
            (None, None) => c_string(&header[157..257]),
        };
        let uid = pax_uid
            .take()
            .map(Ok)
            .unwrap_or_else(|| parse_number(&header[108..116]))?;
        let gid = pax_gid
            .take()
            .map(Ok)
            .unwrap_or_else(|| parse_number(&header[116..124]))?;
        let mode = parse_number(&header[100..108])? as u32 & 0o7777;
        let path = normalize_path(&name);
        let mut entry = Entry {
            path: path.clone(),
            kind: EntryKind::File,
            mode,
            uid,
            gid,
            size,
            offset: position,
            link: String::new(),
            dev_major: 0,
            dev_minor: 0,
            digest: String::new(),
        };
        match typeflag {
            b'0' | 0 | b'7' => {
                let mut hasher = Sha256::new();
                let mut remaining = size;
                let mut buffer = vec![0u8; 64 * 1024];
                while remaining > 0 {
                    let chunk = remaining.min(buffer.len() as u64) as usize;
                    reader.read_exact(&mut buffer[..chunk]).map_err(|error| {
                        format!("truncated tar in '{}': {error}", tar.display())
                    })?;
                    hasher.update(&buffer[..chunk]);
                    remaining -= chunk as u64;
                }
                entry.digest = gaia_process::hex(&hasher.finalize());
            }
            b'5' => {
                entry.kind = EntryKind::Dir;
                skip_data(&mut reader, size)?;
            }
            b'2' => {
                entry.kind = EntryKind::Symlink;
                entry.link = link_name;
                skip_data(&mut reader, size)?;
            }
            b'1' => {
                entry.kind = EntryKind::Hardlink;
                entry.link = normalize_path(&link_name);
                skip_data(&mut reader, size)?;
            }
            b'3' | b'4' => {
                entry.kind = if typeflag == b'3' {
                    EntryKind::CharDev
                } else {
                    EntryKind::BlockDev
                };
                entry.dev_major = parse_number(&header[329..337])? as u32;
                entry.dev_minor = parse_number(&header[337..345])? as u32;
                skip_data(&mut reader, size)?;
            }
            b'6' => {
                entry.kind = EntryKind::Fifo;
                skip_data(&mut reader, size)?;
            }
            other => {
                return Err(format!(
                    "unsupported tar entry type '{}' for '{name}' in '{}'",
                    other as char,
                    tar.display()
                ));
            }
        }
        // The data was consumed above (hashed, or skipped); only padding is left.
        skip_padding(&mut reader, size)?;
        position += data_blocks;
        if path.is_empty() && entry.kind != EntryKind::Dir {
            return Err(format!(
                "tar entry '{name}' in '{}' is not a directory",
                tar.display()
            ));
        }
        entries.insert(path, entry);
    }
    Ok(entries)
}

/// Reads `buffer` in full. `false` on a clean end of file before any byte.
fn read_full(reader: &mut impl Read, buffer: &mut [u8]) -> Result<bool, String> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err("truncated tar header".into()),
            Ok(read) => filled += read,
            Err(error) => return Err(format!("failed to read tar: {error}")),
        }
    }
    Ok(true)
}

fn skip_data(reader: &mut impl Read, size: u64) -> Result<(), String> {
    let copied = io::copy(&mut reader.by_ref().take(size), &mut io::sink())
        .map_err(|error| format!("failed to read tar data: {error}"))?;
    if copied != size {
        return Err("truncated tar data".into());
    }
    Ok(())
}

fn skip_padding(reader: &mut impl Read, size: u64) -> Result<(), String> {
    let padding = (BLOCK as u64 - size % BLOCK as u64) % BLOCK as u64;
    if padding > 0 {
        let mut pad = [0u8; BLOCK];
        reader
            .read_exact(&mut pad[..padding as usize])
            .map_err(|error| format!("truncated tar padding: {error}"))?;
    }
    Ok(())
}

fn verify_checksum(header: &[u8; BLOCK], tar: &Path) -> Result<(), String> {
    let stored = parse_number(&header[148..156])?;
    let computed: u64 = header
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            if (148..156).contains(&index) {
                u64::from(b' ')
            } else {
                u64::from(*byte)
            }
        })
        .sum();
    if stored != computed {
        return Err(format!("bad tar header checksum in '{}'", tar.display()));
    }
    Ok(())
}

/// A numeric header field: octal, or base-256 when the high bit is set.
fn parse_number(field: &[u8]) -> Result<u64, String> {
    if field.first().is_some_and(|byte| byte & 0x80 != 0) {
        let mut value = u64::from(field[0] & 0x7f);
        for byte in &field[1..] {
            value = value
                .checked_mul(256)
                .and_then(|value| value.checked_add(u64::from(*byte)))
                .ok_or("tar numeric field overflows")?;
        }
        return Ok(value);
    }
    let text: String = field
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as char)
        .collect();
    let trimmed = text.trim_matches(|c: char| c == ' ' || c == '\0');
    if trimmed.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(trimmed, 8).map_err(|_| format!("bad tar numeric field '{trimmed}'"))
}

fn c_string(field: &[u8]) -> String {
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

/// The name of a ustar header: `prefix/name` when the prefix is set.
fn header_name(header: &[u8; BLOCK]) -> String {
    let name = c_string(&header[0..100]);
    let magic = &header[257..263];
    if magic == b"ustar\0" || magic == b"ustar " {
        let prefix = c_string(&header[345..500]);
        if !prefix.is_empty() {
            return format!("{prefix}/{name}");
        }
    }
    name
}

/// `len key=value\n` records, where `len` counts the whole record.
fn pax_records(body: &[u8]) -> Result<Vec<(String, String)>, String> {
    let mut records = Vec::new();
    let mut rest = body;
    while !rest.is_empty() {
        let space = rest
            .iter()
            .position(|byte| *byte == b' ')
            .ok_or("bad PAX record")?;
        let length: usize = std::str::from_utf8(&rest[..space])
            .ok()
            .and_then(|text| text.parse().ok())
            .ok_or("bad PAX record length")?;
        if length == 0 || length > rest.len() {
            return Err("bad PAX record length".into());
        }
        let record = &rest[space + 1..length.saturating_sub(1)];
        let equals = record
            .iter()
            .position(|byte| *byte == b'=')
            .ok_or("bad PAX record")?;
        records.push((
            String::from_utf8_lossy(&record[..equals]).into_owned(),
            String::from_utf8_lossy(&record[equals + 1..]).into_owned(),
        ));
        rest = &rest[length..];
    }
    Ok(records)
}

/// Copies `size` bytes of a regular file's data, starting at `offset` in `tar`.
pub(crate) fn copy_data(
    tar: &mut File,
    offset: u64,
    size: u64,
    out: &mut impl Write,
) -> Result<(), String> {
    tar.seek(SeekFrom::Start(offset))
        .map_err(|error| format!("failed to seek in source tar: {error}"))?;
    let copied = io::copy(&mut tar.take(size), out)
        .map_err(|error| format!("failed to copy tar data: {error}"))?;
    if copied != size {
        return Err("source tar data is truncated".into());
    }
    Ok(())
}

/// A deterministic tar writer. Every entry gets the same mtime; owner and mode
/// come from the entry.
pub(crate) struct LayerTarWriter<W: Write> {
    inner: W,
    mtime: u64,
}

impl<W: Write> LayerTarWriter<W> {
    pub(crate) fn new(inner: W, mtime: u64) -> Self {
        Self { inner, mtime }
    }

    /// A directory entry. The name is `usr/bin` (the trailing slash is added).
    pub(crate) fn dir(&mut self, path: &str, mode: u32, uid: u64, gid: u64) -> Result<(), String> {
        let name = if path.is_empty() {
            "./".to_string()
        } else {
            format!("{path}/")
        };
        self.header(&name, b'5', mode, uid, gid, 0, "", (0, 0))
    }

    /// A regular file whose data `copy` writes (exactly `size` bytes).
    pub(crate) fn file(
        &mut self,
        path: &str,
        mode: u32,
        uid: u64,
        gid: u64,
        size: u64,
        copy: impl FnOnce(&mut W) -> Result<(), String>,
    ) -> Result<(), String> {
        self.header(path, b'0', mode, uid, gid, size, "", (0, 0))?;
        copy(&mut self.inner)?;
        self.pad(size)
    }

    pub(crate) fn symlink(
        &mut self,
        path: &str,
        target: &str,
        uid: u64,
        gid: u64,
    ) -> Result<(), String> {
        self.header(path, b'2', 0o777, uid, gid, 0, target, (0, 0))
    }

    pub(crate) fn hardlink(
        &mut self,
        path: &str,
        target: &str,
        mode: u32,
        uid: u64,
        gid: u64,
    ) -> Result<(), String> {
        self.header(path, b'1', mode, uid, gid, 0, target, (0, 0))
    }

    pub(crate) fn device(
        &mut self,
        path: &str,
        block: bool,
        mode: u32,
        uid: u64,
        gid: u64,
        major_minor: (u32, u32),
    ) -> Result<(), String> {
        let flag = if block { b'4' } else { b'3' };
        self.header(path, flag, mode, uid, gid, 0, "", major_minor)
    }

    pub(crate) fn fifo(&mut self, path: &str, mode: u32, uid: u64, gid: u64) -> Result<(), String> {
        self.header(path, b'6', mode, uid, gid, 0, "", (0, 0))
    }

    /// Writes the two zero blocks that end the archive and flushes.
    pub(crate) fn finish(mut self) -> Result<W, String> {
        self.inner
            .write_all(&[0u8; BLOCK * 2])
            .and_then(|()| self.inner.flush())
            .map_err(|error| format!("failed to finish layer tar: {error}"))?;
        Ok(self.inner)
    }

    fn pad(&mut self, size: u64) -> Result<(), String> {
        let padding = (BLOCK as u64 - size % BLOCK as u64) % BLOCK as u64;
        self.inner
            .write_all(&vec![0u8; padding as usize])
            .map_err(|error| format!("failed to write layer tar padding: {error}"))
    }

    #[allow(clippy::too_many_arguments)]
    fn header(
        &mut self,
        name: &str,
        flag: u8,
        mode: u32,
        uid: u64,
        gid: u64,
        size: u64,
        link: &str,
        dev: (u32, u32),
    ) -> Result<(), String> {
        if size > SIZE_MAX || uid > ID_MAX || gid > ID_MAX {
            return Err(format!(
                "layer entry '{name}' is too large or has an id too big for ustar"
            ));
        }
        let mut pax = Vec::new();
        let mut field_name = name.to_string();
        let mut prefix = String::new();
        if name.len() > NAME_MAX {
            match split_prefix(name) {
                Some((head, tail)) => {
                    prefix = head;
                    field_name = tail;
                }
                None => pax.push(("path", name.to_string())),
            }
        }
        if link.len() > NAME_MAX {
            pax.push(("linkpath", link.to_string()));
        }
        if !pax.is_empty() {
            let body = pax_body(&pax);
            let pax_name = format!("PaxHeaders/{}", short(name));
            self.raw_header(
                &pax_name,
                b'x',
                0o644,
                0,
                0,
                body.len() as u64,
                "",
                (0, 0),
                "",
            )?;
            self.inner
                .write_all(&body)
                .map_err(|error| format!("failed to write PAX record: {error}"))?;
            self.pad(body.len() as u64)?;
        }
        let link_field = if link.len() > NAME_MAX {
            &link[..NAME_MAX]
        } else {
            link
        };
        let name_field = if field_name.len() > NAME_MAX {
            &field_name[..NAME_MAX]
        } else {
            &field_name
        };
        self.raw_header(
            name_field, flag, mode, uid, gid, size, link_field, dev, &prefix,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn raw_header(
        &mut self,
        name: &str,
        flag: u8,
        mode: u32,
        uid: u64,
        gid: u64,
        size: u64,
        link: &str,
        dev: (u32, u32),
        prefix: &str,
    ) -> Result<(), String> {
        let mut header = [0u8; BLOCK];
        put_str(&mut header[0..100], name);
        put_octal(&mut header[100..108], u64::from(mode & 0o7777), 8);
        put_octal(&mut header[108..116], uid, 8);
        put_octal(&mut header[116..124], gid, 8);
        put_octal(&mut header[124..136], size, 12);
        put_octal(&mut header[136..148], self.mtime, 12);
        header[156] = flag;
        put_str(&mut header[157..257], link);
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        put_octal(&mut header[329..337], u64::from(dev.0), 8);
        put_octal(&mut header[337..345], u64::from(dev.1), 8);
        put_str(&mut header[345..500], prefix);
        for byte in &mut header[148..156] {
            *byte = b' ';
        }
        let sum: u64 = header.iter().map(|byte| u64::from(*byte)).sum();
        let checksum = format!("{sum:06o}\0 ");
        header[148..156].copy_from_slice(checksum.as_bytes());
        self.inner
            .write_all(&header)
            .map_err(|error| format!("failed to write layer tar header for '{name}': {error}"))
    }
}

/// Splits `name` into a ustar prefix and name that both fit, at a slash.
fn split_prefix(name: &str) -> Option<(String, String)> {
    name.match_indices('/').find_map(|(index, _)| {
        let (head, tail) = (&name[..index], &name[index + 1..]);
        (head.len() <= PREFIX_MAX && !tail.is_empty() && tail.len() <= NAME_MAX)
            .then(|| (head.to_string(), tail.to_string()))
    })
}

fn short(name: &str) -> String {
    name.chars().take(90).collect()
}

fn pax_body(records: &[(&str, String)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (key, value) in records {
        let rest = format!(" {key}={value}\n");
        // The record's length counts its own digits: find the width that fits.
        let mut digits = 1;
        let length = loop {
            let total = rest.len() + digits;
            let width = total.to_string().len();
            if width == digits {
                break total;
            }
            digits = width;
        };
        body.extend_from_slice(format!("{length}{rest}").as_bytes());
    }
    body
}

fn put_str(field: &mut [u8], value: &str) {
    let bytes = value.as_bytes();
    let length = bytes.len().min(field.len());
    field[..length].copy_from_slice(&bytes[..length]);
}

/// An octal field of `width` bytes: digits, then a NUL.
fn put_octal(field: &mut [u8], value: u64, width: usize) {
    let text = format!("{value:0width$o}", width = width - 1);
    field[..width - 1].copy_from_slice(text.as_bytes());
    field[width - 1] = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn long_names_round_trip_through_pax_and_prefix() {
        let long = format!("usr/share/{}/file.txt", "d".repeat(120));
        let mut out = Vec::new();
        {
            let mut writer = LayerTarWriter::new(&mut out, 0);
            writer
                .file(&long, 0o640, 5, 7, 3, |w| {
                    w.write_all(b"abc").map_err(|error| error.to_string())
                })
                .expect("file");
            writer.finish().expect("finish");
        }
        let dir = gaia_testkit::TempDir::new("layers-tar-long");
        let tar = dir.path().join("long.tar");
        fs::write(&tar, &out).expect("write");
        let entries = read_entries(&tar).expect("read");
        let entry = entries.get(&long).expect("long name kept");
        assert_eq!((entry.uid, entry.gid, entry.mode), (5, 7, 0o640));
        assert_eq!(entry.size, 3);
        assert_eq!(entry.digest, gaia_process::sha256_hex_bytes(b"abc"));
    }

    #[test]
    fn pax_length_counts_itself() {
        let body = pax_body(&[("path", "x".repeat(200))]);
        let parsed = pax_records(&body).expect("parse");
        assert_eq!(parsed, vec![("path".to_string(), "x".repeat(200))]);
    }

    #[test]
    fn normalize_strips_dot_slash_and_slashes() {
        assert_eq!(normalize_path("./usr/bin/"), "usr/bin");
        assert_eq!(normalize_path("./"), "");
        assert_eq!(normalize_path("."), "");
    }
}
