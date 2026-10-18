use std::io::{Read, Write};

/// ustar block size.
pub(super) const TAR_BLOCK: usize = 512;
/// Longest name the ustar `name` field holds on its own. A longer name is
/// split into the `prefix` field (up to [`TAR_PREFIX_MAX`] bytes) when it can
/// be, and otherwise carried by a PAX `path` record.
pub(super) const TAR_NAME_MAX: usize = 100;
/// Longest directory part the ustar `prefix` field holds.
const TAR_PREFIX_MAX: usize = 155;
/// Longest member name the writer accepts; PAX carries names past ustar.
pub(super) const TAR_PATH_MAX: usize = 4096;
/// Largest size the 11-digit octal size field can store.
const TAR_SIZE_MAX: u64 = 0o77777777777;
const TAR_FILE_MODE: u32 = 0o644;
/// Typeflag of a PAX extended header, which describes the entry after it.
const PAX_HEADER_FLAG: u8 = b'x';

/// The entry kinds the writer emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TarKind {
    File,
    Dir,
    Symlink,
}

impl TarKind {
    fn type_flag(self) -> u8 {
        match self {
            Self::File => b'0',
            Self::Dir => b'5',
            Self::Symlink => b'2',
        }
    }
}

/// Writes a deterministic POSIX ustar stream: mtime 0, uid/gid 0, empty
/// user/group names. Names and link targets that ustar cannot hold use the
/// `prefix` field or PAX records. Regular files take the mode given (0644 or
/// 0755 in practice), directories 0755, symlinks 0777.
pub(super) struct TarWriter<W: Write> {
    inner: W,
}

impl<W: Write> TarWriter<W> {
    pub(super) fn new(inner: W) -> Self {
        Self { inner }
    }

    /// Appends a regular file with mode 0644; see [`TarWriter::append_file`].
    pub(super) fn append(
        &mut self,
        name: &str,
        size: u64,
        reader: &mut impl Read,
        observe: impl FnMut(&[u8]),
    ) -> Result<(), String> {
        self.append_file(name, TAR_FILE_MODE, size, reader, observe)
    }

    /// Appends a file entry whose contents come from `reader`, which must
    /// yield exactly `size` bytes. Each chunk is also passed to `observe`.
    pub(super) fn append_file(
        &mut self,
        name: &str,
        mode: u32,
        size: u64,
        reader: &mut impl Read,
        mut observe: impl FnMut(&[u8]),
    ) -> Result<(), String> {
        self.write_header(name, size, mode, TarKind::File, "")?;
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

    /// Appends a directory entry; `name` should end in `/`.
    pub(super) fn append_dir(&mut self, name: &str) -> Result<(), String> {
        self.write_header(name, 0, 0o755, TarKind::Dir, "")
    }

    /// Appends a symbolic link entry pointing at `target`, stored as is.
    pub(super) fn append_symlink(&mut self, name: &str, target: &str) -> Result<(), String> {
        self.write_header(name, 0, 0o777, TarKind::Symlink, target)
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

    fn write_header(
        &mut self,
        name: &str,
        size: u64,
        mode: u32,
        kind: TarKind,
        link: &str,
    ) -> Result<(), String> {
        let blocks = entry_header_blocks(name, size, mode, kind, link)?;
        self.inner
            .write_all(&blocks)
            .map_err(|error| format!("failed to write tar header for '{name}': {error}"))
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

/// A regular file header with mode 0644 for a name that fits the ustar
/// fields without PAX records.
#[cfg(test)]
pub(super) fn ustar_header(name: &str, size: u64) -> Result<[u8; TAR_BLOCK], String> {
    let blocks = entry_header_blocks(name, size, TAR_FILE_MODE, TarKind::File, "")?;
    if blocks.len() != TAR_BLOCK {
        return Err(format!("tar member name '{name}' needs PAX records"));
    }
    let mut header = [0u8; TAR_BLOCK];
    header.copy_from_slice(&blocks);
    Ok(header)
}

/// The blocks that precede an entry's data: a PAX extended header carrying
/// the `path` and/or `linkpath` that the ustar fields cannot hold, then the
/// ustar header. The result is a whole number of blocks.
fn entry_header_blocks(
    name: &str,
    size: u64,
    mode: u32,
    kind: TarKind,
    link: &str,
) -> Result<Vec<u8>, String> {
    if name.is_empty() || name.len() > TAR_PATH_MAX {
        return Err(format!(
            "tar member name '{name}' must be 1-{TAR_PATH_MAX} bytes"
        ));
    }
    if size > TAR_SIZE_MAX {
        return Err(format!(
            "tar member '{name}' is {size} bytes; ustar entries are limited to {TAR_SIZE_MAX} bytes"
        ));
    }
    let mut records: Vec<(&str, &str)> = Vec::new();
    let (prefix, base): (&[u8], &[u8]) = match ustar_split(name) {
        Some(split) => split,
        None => {
            records.push(("path", name));
            (&[], &name.as_bytes()[..TAR_NAME_MAX])
        }
    };
    let link_field: &[u8] = if link.len() <= TAR_NAME_MAX {
        link.as_bytes()
    } else {
        records.push(("linkpath", link));
        &link.as_bytes()[..TAR_NAME_MAX]
    };
    let mut blocks = Vec::new();
    if !records.is_empty() {
        let data = pax_records(&records);
        blocks.extend_from_slice(&ustar_block(
            pax_header_name(name).as_bytes(),
            &[],
            0o644,
            data.len() as u64,
            PAX_HEADER_FLAG,
            &[],
        ));
        blocks.extend_from_slice(&data);
        let remainder = data.len() % TAR_BLOCK;
        if remainder != 0 {
            blocks.resize(blocks.len() + TAR_BLOCK - remainder, 0);
        }
    }
    blocks.extend_from_slice(&ustar_block(
        base,
        prefix,
        mode,
        size,
        kind.type_flag(),
        link_field,
    ));
    Ok(blocks)
}

/// Splits `name` into the ustar `prefix` and `name` fields at a `/`, when
/// the name is longer than the `name` field and both parts fit. A name that
/// fits the `name` field alone is not split.
fn ustar_split(name: &str) -> Option<(&[u8], &[u8])> {
    let bytes = name.as_bytes();
    if bytes.len() <= TAR_NAME_MAX {
        return Some((&[], bytes));
    }
    // The rightmost separator with a prefix that fits keeps the base name
    // shortest, so it is the one to take.
    bytes
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, byte)| **byte == b'/')
        .find_map(|(index, _)| {
            let (prefix, rest) = (&bytes[..index], &bytes[index + 1..]);
            (prefix.len() <= TAR_PREFIX_MAX && !rest.is_empty() && rest.len() <= TAR_NAME_MAX)
                .then_some((prefix, rest))
        })
}

/// The name of a PAX header entry: a fixed prefix and the end of `name`, so
/// listings show something recognisable. Always fits the `name` field.
fn pax_header_name(name: &str) -> String {
    let mut start = name.len().saturating_sub(80);
    while !name.is_char_boundary(start) {
        start += 1;
    }
    let mut header = format!("PaxHeaders/{}", &name[start..]);
    header.truncate(TAR_NAME_MAX);
    header
}

/// PAX records `<length> <key>=<value>\n`, where the length counts itself.
fn pax_records(records: &[(&str, &str)]) -> Vec<u8> {
    let mut data = Vec::new();
    for (key, value) in records {
        let body = format!(" {key}={value}\n");
        let mut length = body.len() + 1;
        loop {
            let next = body.len() + length.to_string().len();
            if next == length {
                break;
            }
            length = next;
        }
        data.extend_from_slice(format!("{length}{body}").as_bytes());
    }
    data
}

/// One ustar header block. `name`, `prefix` and `link` must fit their fields.
fn ustar_block(
    name: &[u8],
    prefix: &[u8],
    mode: u32,
    size: u64,
    flag: u8,
    link: &[u8],
) -> [u8; TAR_BLOCK] {
    let mut header = [0u8; TAR_BLOCK];
    header[..name.len()].copy_from_slice(name);
    write_octal(&mut header[100..108], mode as u64);
    write_octal(&mut header[108..116], 0); // uid
    write_octal(&mut header[116..124], 0); // gid
    write_octal(&mut header[124..136], size);
    write_octal(&mut header[136..148], 0); // mtime
    header[156] = flag;
    header[157..157 + link.len()].copy_from_slice(link);
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    // uname/gname (265..329) and devmajor/devminor (329..345) stay zero.
    header[345..345 + prefix.len()].copy_from_slice(prefix);
    header[148..156].copy_from_slice(b"        ");
    let checksum: u32 = header.iter().map(|byte| *byte as u32).sum();
    let digits = format!("{checksum:06o}");
    header[148..154].copy_from_slice(digits.as_bytes());
    header[154] = 0;
    header[155] = b' ';
    header
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
    fn dir_symlink_and_mode_headers_carry_their_type() {
        let mut writer = TarWriter::new(Vec::new());
        writer.append_dir("boot/").expect("dir");
        writer
            .append_symlink("boot/link", "target.dtbo")
            .expect("symlink");
        writer
            .append_file("boot/run.sh", 0o755, 0, &mut &b""[..], |_| {})
            .expect("exec file");
        let bytes = writer.finish().expect("finish");
        assert_eq!(bytes[156], b'5');
        assert_eq!(&bytes[100..108], b"0000755\0");
        assert_eq!(&bytes[124..136], b"00000000000\0");
        assert_eq!(bytes[TAR_BLOCK + 156], b'2');
        assert_eq!(
            &bytes[TAR_BLOCK + 157..TAR_BLOCK + 157 + 11],
            b"target.dtbo"
        );
        assert_eq!(bytes[2 * TAR_BLOCK + 156], b'0');
        assert_eq!(
            &bytes[2 * TAR_BLOCK + 100..2 * TAR_BLOCK + 108],
            b"0000755\0"
        );
    }

    #[test]
    fn ustar_header_rejects_names_that_need_pax_and_huge_sizes() {
        assert!(ustar_header(&"n".repeat(100), 0).is_ok());
        assert!(ustar_header(&"n".repeat(101), 0).is_err());
        assert!(ustar_header("", 0).is_err());
        assert!(ustar_header("big", TAR_SIZE_MAX + 1).is_err());
        assert!(
            entry_header_blocks(&"n".repeat(TAR_PATH_MAX + 1), 0, 0o644, TarKind::File, "")
                .is_err()
        );
    }

    #[test]
    fn long_names_split_into_prefix_or_use_pax() {
        // 250 bytes with separators: split into prefix and name fields.
        let split_name = format!("{}/file.txt", ["abcdefgh"; 27].join("/"));
        assert!(split_name.len() > TAR_NAME_MAX && split_name.len() < 260);
        let (prefix, base) = ustar_split(&split_name).expect("split");
        assert!(prefix.len() <= TAR_PREFIX_MAX && base.len() <= TAR_NAME_MAX);
        let blocks = entry_header_blocks(&split_name, 0, 0o644, TarKind::File, "").expect("entry");
        assert_eq!(blocks.len(), TAR_BLOCK, "a split name needs no PAX block");
        assert_eq!(&blocks[345..345 + prefix.len()], prefix);

        // No separator to split at: a PAX `path` record carries the name.
        let flat = "x".repeat(250);
        let blocks = entry_header_blocks(&flat, 0, 0o644, TarKind::File, "").expect("entry");
        // PAX header block, its data (one padded block), then the header.
        assert_eq!(blocks.len(), 3 * TAR_BLOCK);
        assert_eq!(blocks[156], PAX_HEADER_FLAG);
        let pax = String::from_utf8_lossy(&blocks[TAR_BLOCK..2 * TAR_BLOCK]).to_string();
        assert!(pax.contains(&format!(" path={flat}\n")), "{pax}");
        assert_eq!(blocks[2 * TAR_BLOCK + 156], b'0');

        // A long link target takes a PAX `linkpath` record.
        let target = "t/".repeat(80);
        let blocks = entry_header_blocks("l", 0, 0o777, TarKind::Symlink, &target).expect("link");
        assert_eq!(blocks[156], PAX_HEADER_FLAG);
        let pax = String::from_utf8_lossy(&blocks[TAR_BLOCK..2 * TAR_BLOCK]).to_string();
        assert!(pax.contains(&format!(" linkpath={target}\n")), "{pax}");
        assert_eq!(blocks[2 * TAR_BLOCK + 156], b'2');
    }

    #[test]
    fn pax_records_count_their_own_length() {
        let data = pax_records(&[("path", &"p".repeat(120))]);
        let text = String::from_utf8(data).expect("utf8");
        let length: usize = text
            .split(' ')
            .next()
            .expect("length")
            .parse()
            .expect("number");
        assert_eq!(length, text.len());
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

    #[test]
    fn long_member_names_list_with_tar() {
        let dir = gaia_testkit::TempDir::new("tar-long-names");
        let long = format!("{}/{}", "d".repeat(120), "f".repeat(120));
        let flat = "g".repeat(150);
        let link_target = format!("{}/target", "t".repeat(130));
        let mut writer = TarWriter::new(Vec::new());
        writer.append_dir("tree/").expect("dir");
        writer
            .append(&long, 2, &mut &b"hi"[..], |_| {})
            .expect("long");
        writer
            .append(&flat, 1, &mut &b"x"[..], |_| {})
            .expect("flat");
        writer
            .append_symlink("tree/link", &link_target)
            .expect("symlink");
        let bytes = writer.finish().expect("finish");
        let archive = dir.path().join("long.tar");
        std::fs::write(&archive, &bytes).expect("write archive");
        let listing = std::process::Command::new("tar")
            .arg("-tf")
            .arg(&archive)
            .output()
            .expect("run tar");
        assert!(listing.status.success(), "{listing:?}");
        let names = String::from_utf8_lossy(&listing.stdout).to_string();
        assert!(names.lines().any(|line| line == long), "{names}");
        assert!(names.lines().any(|line| line == flat), "{names}");
        let verbose = std::process::Command::new("tar")
            .arg("-tvf")
            .arg(&archive)
            .output()
            .expect("run tar");
        let text = String::from_utf8_lossy(&verbose.stdout).to_string();
        assert!(text.contains(&format!("-> {link_target}")), "{text}");
    }
}
