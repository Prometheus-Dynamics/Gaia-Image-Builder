use std::io::{Read, Write};

/// ustar block size.
pub(super) const TAR_BLOCK: usize = 512;
/// Longest name the ustar `name` field holds; the `prefix` field is unused.
pub(super) const TAR_NAME_MAX: usize = 100;
/// Largest size the 11-digit octal size field can store.
const TAR_SIZE_MAX: u64 = 0o77777777777;
const TAR_FILE_MODE: u32 = 0o644;

/// Writes a deterministic POSIX ustar stream: regular files only, mtime 0,
/// uid/gid 0, empty user/group names and mode 0644.
pub(super) struct TarWriter<W: Write> {
    inner: W,
}

impl<W: Write> TarWriter<W> {
    pub(super) fn new(inner: W) -> Self {
        Self { inner }
    }

    /// Appends a file entry whose contents come from `reader`, which must
    /// yield exactly `size` bytes. Each chunk is also passed to `observe`.
    pub(super) fn append(
        &mut self,
        name: &str,
        size: u64,
        reader: &mut impl Read,
        mut observe: impl FnMut(&[u8]),
    ) -> Result<(), String> {
        self.inner
            .write_all(&ustar_header(name, size)?)
            .map_err(|error| format!("failed to write tar header for '{name}': {error}"))?;
        let mut buffer = vec![0u8; 64 * 1024];
        let mut written = 0u64;
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|error| format!("failed to read tar member '{name}': {error}"))?;
            if read == 0 {
                break;
            }
            written += read as u64;
            if written > size {
                break;
            }
            observe(&buffer[..read]);
            self.inner
                .write_all(&buffer[..read])
                .map_err(|error| format!("failed to write tar member '{name}': {error}"))?;
        }
        if written != size {
            return Err(format!(
                "tar member '{name}' changed while archiving: expected {size} bytes, read {written}"
            ));
        }
        self.write_padding(size)
    }

    /// Writes the two zero blocks that end the archive and returns the
    /// underlying writer.
    pub(super) fn finish(mut self) -> Result<W, String> {
        self.inner
            .write_all(&[0u8; TAR_BLOCK * 2])
            .and_then(|_| self.inner.flush())
            .map_err(|error| format!("failed to finish tar archive: {error}"))?;
        Ok(self.inner)
    }

    fn write_padding(&mut self, size: u64) -> Result<(), String> {
        let remainder = (size % TAR_BLOCK as u64) as usize;
        if remainder == 0 {
            return Ok(());
        }
        self.inner
            .write_all(&vec![0u8; TAR_BLOCK - remainder])
            .map_err(|error| format!("failed to pad tar member: {error}"))
    }
}

pub(super) fn ustar_header(name: &str, size: u64) -> Result<[u8; TAR_BLOCK], String> {
    if name.is_empty() || name.len() > TAR_NAME_MAX {
        return Err(format!(
            "tar member name '{name}' must be 1-{TAR_NAME_MAX} bytes (ustar name field)"
        ));
    }
    if size > TAR_SIZE_MAX {
        return Err(format!(
            "tar member '{name}' is {size} bytes; ustar entries are limited to {TAR_SIZE_MAX} bytes"
        ));
    }
    let mut header = [0u8; TAR_BLOCK];
    header[..name.len()].copy_from_slice(name.as_bytes());
    write_octal(&mut header[100..108], TAR_FILE_MODE as u64);
    write_octal(&mut header[108..116], 0); // uid
    write_octal(&mut header[116..124], 0); // gid
    write_octal(&mut header[124..136], size);
    write_octal(&mut header[136..148], 0); // mtime
    header[156] = b'0'; // regular file
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    // uname/gname (265..329), devmajor/devminor and prefix stay zero.
    header[148..156].copy_from_slice(b"        ");
    let checksum: u32 = header.iter().map(|byte| *byte as u32).sum();
    let digits = format!("{checksum:06o}");
    header[148..154].copy_from_slice(digits.as_bytes());
    header[154] = 0;
    header[155] = b' ';
    Ok(header)
}

/// Zero-padded octal digits followed by a NUL terminator.
fn write_octal(field: &mut [u8], value: u64) {
    let width = field.len() - 1;
    let digits = format!("{value:0width$o}");
    field[..width].copy_from_slice(digits.as_bytes());
    field[width] = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ustar_header_encodes_fixed_metadata_and_checksum() {
        let header = ustar_header("dir/file.txt", 1234).expect("header");
        assert_eq!(&header[..12], b"dir/file.txt");
        assert_eq!(&header[100..108], b"0000644\0");
        assert_eq!(&header[108..116], b"0000000\0");
        assert_eq!(&header[124..136], b"00000002322\0");
        assert_eq!(&header[136..148], b"00000000000\0");
        assert_eq!(header[156], b'0');
        assert_eq!(&header[257..265], b"ustar\x0000");
        assert!(header[265..329].iter().all(|byte| *byte == 0));
        let mut unsummed = header;
        unsummed[148..156].copy_from_slice(b"        ");
        let expected: u32 = unsummed.iter().map(|byte| *byte as u32).sum();
        let stored = std::str::from_utf8(&header[148..154]).expect("octal");
        assert_eq!(u32::from_str_radix(stored, 8).expect("checksum"), expected);
        assert_eq!(&header[154..156], b"\0 ");
    }

    #[test]
    fn ustar_header_rejects_long_names_and_huge_sizes() {
        assert!(ustar_header(&"n".repeat(100), 0).is_ok());
        assert!(ustar_header(&"n".repeat(101), 0).is_err());
        assert!(ustar_header("", 0).is_err());
        assert!(ustar_header("big", TAR_SIZE_MAX + 1).is_err());
    }

    #[test]
    fn tar_writer_pads_members_and_ends_with_two_zero_blocks() {
        let mut writer = TarWriter::new(Vec::new());
        let mut seen = Vec::new();
        writer
            .append("a", 3, &mut &b"abc"[..], |chunk| {
                seen.extend_from_slice(chunk)
            })
            .expect("append");
        let bytes = writer.finish().expect("finish");
        assert_eq!(seen, b"abc");
        assert_eq!(bytes.len(), TAR_BLOCK * 4);
        assert_eq!(&bytes[TAR_BLOCK..TAR_BLOCK + 3], b"abc");
        assert!(bytes[TAR_BLOCK + 3..].iter().all(|byte| *byte == 0));

        let mut writer = TarWriter::new(Vec::new());
        let error = writer
            .append("short", 5, &mut &b"abc"[..], |_| {})
            .expect_err("short member");
        assert!(error.contains("expected 5 bytes, read 3"), "{error}");
    }
}
