//! A minimal deterministic ustar writer: sorted entries, mtime 0, uid and
//! gid 0, no user or group names.

use std::io::{self, Write};

const BLOCK: usize = 512;

/// One entry: a directory (no data) or a regular file.
pub(crate) struct Entry<'a> {
    pub path: &'a str,
    pub mode: u32,
    pub data: Option<&'a [u8]>,
}

/// Writes `entries`, sorted by path, then the two zero blocks that end a
/// tar. Paths longer than 100 bytes are refused.
pub(crate) fn write(out: &mut impl Write, entries: &mut [Entry<'_>]) -> io::Result<()> {
    entries.sort_by(|a, b| a.path.cmp(b.path));
    for entry in entries.iter() {
        let name = match entry.data {
            Some(_) => entry.path.to_string(),
            None => format!("{}/", entry.path.trim_end_matches('/')),
        };
        if name.len() > 100 {
            return Err(io::Error::other(format!("tar path too long: {name}")));
        }
        let size = entry.data.map_or(0, <[u8]>::len);
        let mut header = [0_u8; BLOCK];
        header[..name.len()].copy_from_slice(name.as_bytes());
        octal(&mut header[100..108], u64::from(entry.mode));
        octal(&mut header[108..116], 0);
        octal(&mut header[116..124], 0);
        octal(&mut header[124..136], size as u64);
        octal(&mut header[136..148], 0);
        header[156] = if entry.data.is_some() { b'0' } else { b'5' };
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        header[148..156].copy_from_slice(b"        ");
        let sum: u32 = header.iter().map(|&byte| u32::from(byte)).sum();
        let checksum = format!("{sum:06o}\0 ");
        header[148..156].copy_from_slice(checksum.as_bytes());
        out.write_all(&header)?;
        if let Some(data) = entry.data {
            out.write_all(data)?;
            let pad = (BLOCK - data.len() % BLOCK) % BLOCK;
            out.write_all(&[0; BLOCK][..pad])?;
        }
    }
    out.write_all(&[0; BLOCK * 2])
}

/// `value` as zero-padded octal filling the field, ending in a NUL.
fn octal(field: &mut [u8], value: u64) {
    let text = format!("{value:0width$o}\0", width = field.len() - 1);
    field.copy_from_slice(text.as_bytes());
}
