//! The container formats of a package: the `ar` archive (written in the GNU
//! style `ar` and opkg-build use, with `/`-terminated member names) and the
//! ustar and PAX tars inside it. The writer side is in `package.rs`; this
//! file reads what it wrote, back, to check it.

use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use super::tar::TAR_BLOCK;

const AR_MAGIC: &[u8] = b"!<arch>\n";
const AR_HEADER_LEN: usize = 60;
/// Mode of every ar member: a regular file, 0644.
const AR_MODE: &str = "100644";

/// Writes an ar archive holding `members` (name, the file whose bytes it
/// holds), in the order given. Times, owners and modes are fixed so the
/// same members give the same bytes.
pub(super) fn write_ar(output: &Path, members: &[(&str, &Path)]) -> Result<(), String> {
    let file = File::create(output)
        .map_err(|error| format!("failed to create '{}': {error}", output.display()))?;
    let mut out = BufWriter::new(file);
    let write = |out: &mut BufWriter<File>, bytes: &[u8]| {
        out.write_all(bytes)
            .map_err(|error| format!("failed to write '{}': {error}", output.display()))
    };
    write(&mut out, AR_MAGIC)?;
    for (name, path) in members {
        let size = std::fs::metadata(path)
            .map_err(|error| format!("failed to stat '{}': {error}", path.display()))?
            .len();
        let member_name = format!("{name}/");
        if member_name.len() > 16 {
            return Err(format!("ar member name '{name}' is longer than 15 bytes"));
        }
        let header = format!(
            "{member_name:<16}{:<12}{:<6}{:<6}{AR_MODE:<8}{size:<10}`\n",
            0, 0, 0
        );
        debug_assert_eq!(header.len(), AR_HEADER_LEN);
        write(&mut out, header.as_bytes())?;
        let mut source = File::open(path)
            .map_err(|error| format!("failed to open '{}': {error}", path.display()))?;
        let copied = io::copy(&mut source, &mut out)
            .map_err(|error| format!("failed to copy '{}': {error}", path.display()))?;
        if copied != size {
            return Err(format!(
                "'{}' changed while it was packed: expected {size} bytes, copied {copied}",
                path.display()
            ));
        }
        if size % 2 == 1 {
            write(&mut out, b"\n")?;
        }
    }
    out.flush()
        .and_then(|()| out.get_ref().sync_all())
        .map_err(|error| format!("failed to finish '{}': {error}", output.display()))
}

/// One member of an ar archive: its name without the trailing `/`, and where
/// its bytes are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ArMember {
    pub(super) name: String,
    pub(super) offset: u64,
    pub(super) size: u64,
}

/// The members of the ar archive at `path`, in file order.
pub(super) fn read_ar(path: &Path) -> Result<Vec<ArMember>, String> {
    let mut file = File::open(path)
        .map_err(|error| format!("failed to open '{}': {error}", path.display()))?;
    let total = file
        .metadata()
        .map_err(|error| format!("failed to stat '{}': {error}", path.display()))?
        .len();
    let mut magic = [0u8; AR_MAGIC.len()];
    file.read_exact(&mut magic)
        .map_err(|_| format!("'{}' is not an ar archive", path.display()))?;
    if magic != AR_MAGIC {
        return Err(format!("'{}' is not an ar archive", path.display()));
    }
    let mut members = Vec::new();
    let mut position = AR_MAGIC.len() as u64;
    while position < total {
        let mut header = [0u8; AR_HEADER_LEN];
        file.read_exact(&mut header).map_err(|_| {
            format!(
                "'{}' ends inside the header of member {}",
                path.display(),
                members.len() + 1
            )
        })?;
        if &header[58..60] != b"`\n" {
            return Err(format!(
                "'{}' member {} has a bad header terminator",
                path.display(),
                members.len() + 1
            ));
        }
        let raw_name = String::from_utf8_lossy(&header[0..16])
            .trim_end()
            .to_string();
        let Some(name) = raw_name.strip_suffix('/') else {
            return Err(format!(
                "'{}' member {} name '{raw_name}' is not a GNU ar name",
                path.display(),
                members.len() + 1
            ));
        };
        let size: u64 = String::from_utf8_lossy(&header[48..58])
            .trim()
            .parse()
            .map_err(|_| format!("'{}' member '{name}' has a bad size field", path.display()))?;
        let offset = position + AR_HEADER_LEN as u64;
        if offset + size > total {
            return Err(format!(
                "'{}' member '{name}' runs past the end of the file",
                path.display()
            ));
        }
        members.push(ArMember {
            name: name.to_string(),
            offset,
            size,
        });
        let padded = size + size % 2;
        file.seek(SeekFrom::Start(offset + padded))
            .map_err(|error| format!("failed to seek in '{}': {error}", path.display()))?;
        position = offset + padded;
    }
    Ok(members)
}

/// Copies the bytes of `member` from the ar archive at `archive` to `dest`.
pub(super) fn extract_member(archive: &Path, member: &ArMember, dest: &Path) -> Result<(), String> {
    let mut file = File::open(archive)
        .map_err(|error| format!("failed to open '{}': {error}", archive.display()))?;
    file.seek(SeekFrom::Start(member.offset))
        .map_err(|error| format!("failed to seek in '{}': {error}", archive.display()))?;
    let mut out = File::create(dest)
        .map_err(|error| format!("failed to create '{}': {error}", dest.display()))?;
    let copied = io::copy(&mut (&file).take(member.size), &mut out)
        .map_err(|error| format!("failed to read member '{}': {error}", member.name))?;
    if copied != member.size {
        return Err(format!("member '{}' is cut short", member.name));
    }
    Ok(())
}

/// The kind of a tar entry the package writer emits, or `Other` for a kind it
/// does not (devices, fifos).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TarEntryKind {
    File,
    Dir,
    Symlink,
    Hardlink,
    Other,
}

/// One tar entry: its name (PAX `path` applied), kind, mode, size, link
/// target, and where its data starts in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TarEntry {
    pub(super) name: String,
    pub(super) kind: TarEntryKind,
    pub(super) mode: u32,
    pub(super) size: u64,
    pub(super) link: String,
    pub(super) data_offset: u64,
}

/// Lists the entries of the uncompressed tar at `path`, in order. Stops at
/// the zero block that ends the archive. Checks each header's checksum.
pub(super) fn list_tar(path: &Path) -> Result<Vec<TarEntry>, String> {
    let mut file = File::open(path)
        .map_err(|error| format!("failed to open '{}': {error}", path.display()))?;
    let bad = |what: &str| format!("'{}' is not a valid tar: {what}", path.display());
    let mut entries = Vec::new();
    let mut pax_path: Option<String> = None;
    let mut pax_link: Option<String> = None;
    loop {
        let mut header = [0u8; TAR_BLOCK];
        if file.read_exact(&mut header).is_err() {
            // The end of the file: a tar that stops before its zero blocks is
            // read to its last entry. A cut-short entry fails its own read.
            break;
        }
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let checksum = octal(&header[148..156]).ok_or_else(|| bad("bad checksum field"))?;
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
        if computed != checksum {
            return Err(bad("header checksum does not match"));
        }
        let size = octal(&header[124..136]).ok_or_else(|| bad("bad size field"))?;
        let typeflag = header[156];
        let data_len = size + (TAR_BLOCK as u64 - size % TAR_BLOCK as u64) % TAR_BLOCK as u64;
        if typeflag == b'x' || typeflag == b'g' {
            let mut data = vec![0u8; size as usize];
            file.read_exact(&mut data)
                .map_err(|_| bad("PAX header is cut short"))?;
            file.seek(SeekFrom::Current((data_len - size) as i64))
                .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
            if typeflag == b'x' {
                for (key, value) in pax_records(&data) {
                    match key.as_str() {
                        "path" => pax_path = Some(value),
                        "linkpath" => pax_link = Some(value),
                        _ => {}
                    }
                }
            }
            continue;
        }
        let data_offset = file
            .stream_position()
            .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
        let name = match pax_path.take() {
            Some(name) => name,
            None => {
                let base = cstring(&header[0..100]);
                if &header[257..262] == b"ustar" {
                    let prefix = cstring(&header[345..500]);
                    if prefix.is_empty() {
                        base
                    } else {
                        format!("{prefix}/{base}")
                    }
                } else {
                    base
                }
            }
        };
        let link = pax_link
            .take()
            .unwrap_or_else(|| cstring(&header[157..257]));
        let kind = match typeflag {
            b'0' | 0 => TarEntryKind::File,
            b'5' => TarEntryKind::Dir,
            b'2' => TarEntryKind::Symlink,
            b'1' => TarEntryKind::Hardlink,
            _ => TarEntryKind::Other,
        };
        let mode = octal(&header[100..108]).unwrap_or(0) as u32;
        entries.push(TarEntry {
            name,
            kind,
            mode,
            size,
            link,
            data_offset,
        });
        file.seek(SeekFrom::Current(data_len as i64))
            .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
    }
    Ok(entries)
}

/// The bytes of `entry`, read from the tar at `path`.
pub(super) fn read_tar_entry(path: &Path, entry: &TarEntry) -> Result<Vec<u8>, String> {
    let mut file = File::open(path)
        .map_err(|error| format!("failed to open '{}': {error}", path.display()))?;
    file.seek(SeekFrom::Start(entry.data_offset))
        .map_err(|error| format!("failed to seek in '{}': {error}", path.display()))?;
    let mut bytes = vec![0u8; entry.size as usize];
    file.read_exact(&mut bytes)
        .map_err(|_| format!("'{}' entry '{}' is cut short", path.display(), entry.name))?;
    Ok(bytes)
}

/// A NUL-or-space padded octal number, or `None` when it is not one.
fn octal(field: &[u8]) -> Option<u64> {
    let text: String = field
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| char::from(*byte))
        .collect();
    let text = text.trim_matches(|c| c == ' ' || c == '\0');
    if text.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(text, 8).ok()
}

fn cstring(field: &[u8]) -> String {
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

/// The `length key=value\n` records of a PAX header.
fn pax_records(data: &[u8]) -> Vec<(String, String)> {
    let mut records = Vec::new();
    let mut rest = data;
    while !rest.is_empty() {
        let Some(space) = rest.iter().position(|byte| *byte == b' ') else {
            break;
        };
        let Some(length) = std::str::from_utf8(&rest[..space])
            .ok()
            .and_then(|text| text.parse::<usize>().ok())
        else {
            break;
        };
        if length == 0 || length > rest.len() {
            break;
        }
        let record = &rest[space + 1..length - 1];
        if let Ok(text) = std::str::from_utf8(record)
            && let Some((key, value)) = text.split_once('=')
        {
            records.push((key.to_string(), value.to_string()));
        }
        rest = &rest[length..];
    }
    records
}
