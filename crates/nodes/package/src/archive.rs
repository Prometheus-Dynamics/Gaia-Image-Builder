//! The container formats: a tree as a tar, a gzip member, a control tar, and
//! the GNU ar archive opkg and dpkg read. Every writer fixes its times,
//! owners and gzip header, so the same input gives the same bytes.

use flate2::{Compression, write::GzEncoder};
use gaia_types::FileEntry;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// The gzip level of every member.
const GZIP_LEVEL: u32 = 9;
/// Mode of every ar member: a regular file, 0644.
const AR_MODE: &str = "100644";
/// The bytes an ar archive starts with.
const AR_MAGIC: &[u8] = b"!<arch>\n";

/// A gzip member written to a file.
pub(crate) type GzFile = GzEncoder<BufWriter<File>>;

/// A gzip member at `path`, its header fixed (no name, time 0).
pub(crate) fn gz_file(path: &Path) -> io::Result<GzFile> {
    Ok(GzEncoder::new(
        BufWriter::new(File::create(path)?),
        Compression::new(GZIP_LEVEL),
    ))
}

/// Ends a gzip member and flushes it to disk.
pub(crate) fn finish_gz(encoder: GzFile) -> io::Result<()> {
    encoder.finish()?.into_inner()?.sync_all()
}

/// Appends an entry named `name` exactly as given: the tar crate's
/// `append_data` drops a leading `./`, which opkg and dpkg control and data
/// members carry. A name over the header's 100 bytes goes through
/// `append_data` instead, which writes a long-name entry.
fn append_named<W: Write, R: Read>(
    builder: &mut tar::Builder<W>,
    header: &mut tar::Header,
    name: &str,
    data: R,
) -> io::Result<()> {
    if name.len() > 100 {
        return builder.append_data(header, name, data);
    }
    let old = header.as_old_mut();
    old.name = [0; 100];
    old.name[..name.len()].copy_from_slice(name.as_bytes());
    header.set_cksum();
    builder.append(header, data)
}

/// The tree as a tar on `out`: every entry under `prefix`, sorted by path,
/// times 0, numeric owners with no names. Owners and modes come from
/// `entries`; a path not listed is root's, with its stored mode. `root_dir`
/// writes a directory entry for `prefix` first. Returns the writer and the
/// bytes of the regular files, which `Installed-Size` is made from.
pub(crate) fn tree_tar<W: Write>(
    out: W,
    root: &Path,
    entries: &[FileEntry],
    prefix: &str,
    root_dir: bool,
) -> io::Result<(W, u64)> {
    let by_path: BTreeMap<&str, &FileEntry> =
        entries.iter().map(|e| (e.path.as_str(), e)).collect();
    let mut paths = BTreeMap::new();
    walk(root, "", &mut paths)?;
    let mut builder = tar::Builder::new(out);
    builder.mode(tar::HeaderMode::Deterministic);
    if root_dir {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_mode(0o755);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_size(0);
        append_named(&mut builder, &mut header, prefix, io::empty())?;
    }
    let mut installed = 0u64;
    for (path, full) in paths {
        let meta = std::fs::symlink_metadata(&full)?;
        let (uid, gid, mode) = match by_path.get(path.as_str()) {
            Some(e) => (e.uid, e.gid, e.mode & 0o7777),
            None => (0, 0, meta.mode() & 0o7777),
        };
        let mut header = tar::Header::new_gnu();
        header.set_uid(u64::from(uid));
        header.set_gid(u64::from(gid));
        header.set_mtime(0);
        header.set_mode(mode);
        let name = format!("{prefix}{path}");
        let kind = meta.file_type();
        if kind.is_symlink() {
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_mode(0o777);
            header.set_size(0);
            header.set_link_name(std::fs::read_link(&full)?)?;
            append_named(&mut builder, &mut header, &name, io::empty())?;
        } else if kind.is_dir() {
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            append_named(&mut builder, &mut header, &format!("{name}/"), io::empty())?;
        } else {
            installed += meta.len();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(meta.len());
            append_named(&mut builder, &mut header, &name, File::open(&full)?)?;
        }
    }
    Ok((builder.into_inner()?, installed))
}

/// The control tar: each `(name, bytes)` as `./name`, mode 0644, times 0.
pub(crate) fn control_tar<W: Write>(out: W, files: &[(&str, &[u8])]) -> io::Result<W> {
    let mut builder = tar::Builder::new(out);
    builder.mode(tar::HeaderMode::Deterministic);
    for (name, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o644);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_size(bytes.len() as u64);
        append_named(&mut builder, &mut header, &format!("./{name}"), *bytes)?;
    }
    builder.into_inner()
}

/// Writes an ar archive at `output` holding `members` (name, the file whose
/// bytes it holds), in the order given. Names end with `/` as GNU ar's do.
pub(crate) fn write_ar(output: &Path, members: &[(&str, PathBuf)]) -> io::Result<()> {
    let mut out = BufWriter::new(File::create(output)?);
    out.write_all(AR_MAGIC)?;
    for (name, path) in members {
        let size = std::fs::metadata(path)?.len();
        let member = format!("{name}/");
        if member.len() > 16 {
            return Err(io::Error::other(format!(
                "ar member '{name}' has a name longer than 15 bytes"
            )));
        }
        let header = format!(
            "{member:<16}{:<12}{:<6}{:<6}{AR_MODE:<8}{size:<10}`\n",
            0, 0, 0
        );
        out.write_all(header.as_bytes())?;
        io::copy(&mut File::open(path)?, &mut out)?;
        if size % 2 == 1 {
            out.write_all(b"\n")?;
        }
    }
    out.flush()?;
    out.get_ref().sync_all()
}

/// Every path under `dir` with its absolute path, directories included.
fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, PathBuf>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let is_dir = entry.file_type()?.is_dir();
        out.insert(path.clone(), entry.path());
        if is_dir {
            walk(&entry.path(), &path, out)?;
        }
    }
    Ok(())
}
