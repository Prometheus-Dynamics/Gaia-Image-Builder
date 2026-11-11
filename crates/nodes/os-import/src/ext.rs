//! An ext2/3/4 filesystem, read without mounting it.

use crate::disk::Slice;
use crate::out::{Sink, component, join};
use ext4_view::{Ext4, PathBuf as ExtPath};
use std::io;

pub(crate) fn load(slice: Slice) -> io::Result<Ext4> {
    Ext4::load(Box::new(slice)).map_err(|e| io::Error::other(format!("ext4: {e}")))
}

/// Writes everything under `from` (an absolute path in `fs`) into `sink`.
/// A missing `from` writes nothing.
pub(crate) fn copy(fs: &Ext4, from: &str, sink: &mut Sink<'_>) -> io::Result<()> {
    let path = ExtPath::try_from(from).map_err(io::Error::other)?;
    if !fs.exists(&path).map_err(io::Error::other)? {
        return Ok(());
    }
    walk(fs, &path, "", sink)
}

fn walk(fs: &Ext4, dir: &ExtPath, rel: &str, sink: &mut Sink<'_>) -> io::Result<()> {
    let err = |e: ext4_view::Ext4Error| io::Error::other(format!("ext4 {}: {e}", dir.display()));
    let mut entries = Vec::new();
    for entry in fs.read_dir(dir).map_err(err)? {
        let entry = entry.map_err(err)?;
        let name = entry
            .file_name()
            .as_str()
            .map_err(|_| io::Error::other(format!("{}: a name that is not UTF-8", dir.display())))?
            .to_string();
        if name == "." || name == ".." {
            continue;
        }
        entries.push((name, entry.path(), entry.metadata().map_err(err)?));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, path, meta) in entries {
        let rel = join(rel, component(&name)?);
        let (mode, uid, gid) = (u32::from(meta.mode()), meta.uid(), meta.gid());
        let kind = meta.file_type();
        if kind.is_dir() {
            sink.dir(&rel, mode, uid, gid)?;
            walk(fs, &path, &rel, sink)?;
        } else if kind.is_regular_file() {
            sink.file(&rel, &fs.read(&path).map_err(err)?, mode, uid, gid)?;
        } else if kind.is_symlink() {
            let target: std::path::PathBuf = fs.read_link(&path).map_err(err)?.into();
            sink.link(&rel, &target, uid, gid)?;
        } else {
            let kind = if kind.is_char_dev() {
                "char"
            } else if kind.is_block_dev() {
                "block"
            } else if kind.is_fifo() {
                "fifo"
            } else {
                "socket"
            };
            sink.special(&rel, kind, mode, uid, gid);
        }
    }
    Ok(())
}
