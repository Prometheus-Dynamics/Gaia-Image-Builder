//! What an import writes: files with their modes, and the owners and
//! special files the store cannot hold, as `owners.json`.

use gaia_types::FileEntry;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// A device node, FIFO or socket: not stored, recorded for assembly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Special {
    pub path: String,
    /// `char`, `block`, `fifo` or `socket`.
    pub kind: String,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

/// `owners.json`: each tree's paths that are not root's, and the special
/// files found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owners {
    pub rootfs: Vec<FileEntry>,
    pub boot: Vec<FileEntry>,
    pub specials: Vec<Special>,
}

/// Where the files of one tree go, and what is noted on the way.
pub(crate) struct Sink<'a> {
    pub root: &'a Path,
    pub owners: Vec<FileEntry>,
    pub specials: Vec<Special>,
    /// Directories and their modes, set once everything is written.
    dirs: Vec<(String, u32)>,
}

impl<'a> Sink<'a> {
    pub(crate) fn new(root: &'a Path) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        Ok(Self {
            root,
            owners: Vec::new(),
            specials: Vec::new(),
            dirs: Vec::new(),
        })
    }

    fn owner(&mut self, path: &str, mode: u32, uid: u32, gid: u32) {
        if uid != 0 || gid != 0 {
            self.owners.push(FileEntry {
                path: path.to_string(),
                mode,
                uid,
                gid,
            });
        }
    }

    /// A directory; one already written takes the later mode and owner.
    pub(crate) fn dir(&mut self, path: &str, mode: u32, uid: u32, gid: u32) -> io::Result<()> {
        let at = self.root.join(path);
        if !fs::symlink_metadata(&at).is_ok_and(|m| m.is_dir()) {
            fs::create_dir(&at)?;
        }
        self.dirs.push((path.to_string(), mode));
        self.owner(path, mode, uid, gid);
        Ok(())
    }

    pub(crate) fn file(
        &mut self,
        path: &str,
        data: &[u8],
        mode: u32,
        uid: u32,
        gid: u32,
    ) -> io::Result<()> {
        let at = self.root.join(path);
        fs::write(&at, data)?;
        fs::set_permissions(&at, fs::Permissions::from_mode(mode & 0o7777))?;
        self.owner(path, mode & 0o7777, uid, gid);
        Ok(())
    }

    pub(crate) fn link(&mut self, path: &str, target: &Path, uid: u32, gid: u32) -> io::Result<()> {
        std::os::unix::fs::symlink(target, self.root.join(path))?;
        self.owner(path, 0o777, uid, gid);
        Ok(())
    }

    pub(crate) fn special(&mut self, path: &str, kind: &str, mode: u32, uid: u32, gid: u32) {
        self.specials.push(Special {
            path: path.to_string(),
            kind: kind.to_string(),
            mode: mode & 0o7777,
            uid,
            gid,
        });
    }

    /// Sets directory modes, deepest first.
    pub(crate) fn finish(mut self) -> io::Result<(Vec<FileEntry>, Vec<Special>)> {
        // A path given twice keeps its last mode and owner.
        self.dirs.reverse();
        self.dirs.sort_by(|a, b| a.0.cmp(&b.0));
        self.dirs.dedup_by(|a, b| a.0 == b.0);
        for (path, mode) in self.dirs.iter().rev() {
            fs::set_permissions(
                self.root.join(path),
                fs::Permissions::from_mode(mode & 0o7777),
            )?;
        }
        self.owners.reverse();
        self.owners.sort_by(|a, b| a.path.cmp(&b.path));
        self.owners.dedup_by(|a, b| a.path == b.path);
        self.specials.sort_by(|a, b| a.path.cmp(&b.path));
        Ok((self.owners, self.specials))
    }
}

/// `name` as one safe path component.
pub(crate) fn component(name: &str) -> io::Result<&str> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err(io::Error::other(format!("{name:?} is not a file name")));
    }
    Ok(name)
}

pub(crate) fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}
