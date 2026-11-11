//! Applying image layers to a root filesystem, lowest first.
//!
//! Overlay rules, as the OCI image spec gives them:
//! - `.wh.<name>` removes `<name>` from the layers below.
//! - `.wh..wh..opq` in a directory removes everything the layers below put
//!   in it.
//! - A member replaces what is at its path. A directory a member's path
//!   passes through replaces a file or symlink of a lower layer there, so
//!   nothing is ever written through a symlink.
//!
//! Modes are kept as the layer gives them (setuid and sticky bits too);
//! directory modes are set once every layer is applied, so a read-only
//! directory still takes what upper layers put in it. Owners other than
//! root are collected for the tree's entries.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, BufReader, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use tar::EntryType;

/// A path's mode and owner, as the layers left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Owner {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

/// The root filesystem being built.
#[derive(Debug, Default)]
pub(crate) struct Rootfs {
    /// Owners and modes of paths not owned by root, by relative path.
    pub owners: BTreeMap<PathBuf, Owner>,
    /// Directory modes, set at the end.
    dir_modes: BTreeMap<PathBuf, u32>,
}

impl Rootfs {
    /// Applies the layer in `file` (plain, gzip or zstd tar) to `root`.
    pub(crate) fn apply(
        &mut self,
        root: &Path,
        file: &Path,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<()> {
        let mut tar = tar::Archive::new(open(file)?);
        let mut created = BTreeSet::new();
        for entry in tar.entries()? {
            if cancelled() {
                return Err(io::Error::other("cancelled"));
            }
            let mut entry = entry?;
            let kind = entry.header().entry_type();
            if matches!(
                kind,
                EntryType::XGlobalHeader
                    | EntryType::XHeader
                    | EntryType::GNULongName
                    | EntryType::GNULongLink
            ) {
                continue;
            }
            let name = entry.path()?.into_owned();
            let Some(relative) = relative_path(&name)? else {
                continue; // the root itself
            };
            let parent = relative.parent().unwrap_or(Path::new("")).to_path_buf();
            let base = relative
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            self.make_dirs(root, &parent)?;
            if base == ".wh..wh..opq" {
                for child in fs::read_dir(root.join(&parent))? {
                    let child = parent.join(child?.file_name());
                    if !created.contains(&child) {
                        self.remove(root, &child)?;
                    }
                }
                continue;
            }
            if let Some(hidden) = base.strip_prefix(".wh.") {
                self.remove(root, &parent.join(hidden))?;
                continue;
            }
            let target = root.join(&relative);
            // Read where it is used: a link's mode means nothing.
            let link_like = matches!(kind, EntryType::Symlink | EntryType::Link);
            let mode = if link_like {
                0o777
            } else {
                entry.header().mode()? & 0o7777
            };
            match kind {
                EntryType::Directory => {
                    if !fs::symlink_metadata(&target).is_ok_and(|meta| meta.is_dir()) {
                        self.remove(root, &relative)?;
                        fs::create_dir(&target)?;
                    }
                    self.dir_modes.insert(relative.clone(), mode);
                }
                EntryType::Regular | EntryType::Continuous => {
                    self.remove(root, &relative)?;
                    let mut out = fs::File::create(&target)?;
                    io::copy(&mut entry, &mut out)?;
                    fs::set_permissions(&target, fs::Permissions::from_mode(mode))?;
                }
                EntryType::Symlink => {
                    let link = entry
                        .link_name()?
                        .ok_or_else(|| bad(&name, "a symlink with no target"))?;
                    self.remove(root, &relative)?;
                    std::os::unix::fs::symlink(link.as_ref(), &target)?;
                }
                EntryType::Link => {
                    let link = entry
                        .link_name()?
                        .ok_or_else(|| bad(&name, "a hard link with no target"))?;
                    let source = relative_path(&link)?
                        .ok_or_else(|| bad(&name, "a hard link to the root"))?;
                    let mut at = root.to_path_buf();
                    for part in source.components() {
                        at.push(part);
                        if fs::symlink_metadata(&at)?.file_type().is_symlink()
                            && at != root.join(&source)
                        {
                            return Err(bad(&name, "a hard link through a symlink"));
                        }
                    }
                    self.remove(root, &relative)?;
                    fs::hard_link(root.join(&source), &target).map_err(|e| {
                        bad(&name, &format!("a hard link to {}: {e}", source.display()))
                    })?;
                }
                other => return Err(bad(&name, &format!("a {other:?} member is not unpacked"))),
            }
            // An empty owner field reads as root, as GNU tar reads it.
            let (uid, gid) = (
                entry.header().uid().unwrap_or(0),
                entry.header().gid().unwrap_or(0),
            );
            let (uid, gid) = (
                u32::try_from(uid).unwrap_or(u32::MAX),
                u32::try_from(gid).unwrap_or(u32::MAX),
            );
            if uid != 0 || gid != 0 {
                self.owners
                    .insert(relative.clone(), Owner { mode, uid, gid });
            } else {
                self.owners.remove(&relative);
            }
            created.insert(relative);
        }
        Ok(())
    }

    /// Sets directory modes, deepest first.
    pub(crate) fn finish(&mut self, root: &Path) -> io::Result<()> {
        for (dir, mode) in self.dir_modes.iter().rev() {
            let path = root.join(dir);
            if fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir()) {
                fs::set_permissions(&path, fs::Permissions::from_mode(*mode))?;
            }
        }
        for owner in self.owners.iter_mut() {
            if let Some(mode) = self.dir_modes.get(owner.0) {
                owner.1.mode = *mode;
            }
        }
        Ok(())
    }

    /// The directories of `relative` under `root`, made where missing. A
    /// file or symlink in the way is replaced by a directory.
    fn make_dirs(&mut self, root: &Path, relative: &Path) -> io::Result<()> {
        let mut at = PathBuf::new();
        for part in relative.components() {
            at.push(part);
            let path = root.join(&at);
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => {
                    self.remove(root, &at)?;
                    fs::create_dir(&path)?;
                }
                Err(_) => fs::create_dir(&path)?,
            }
        }
        Ok(())
    }

    /// Removes whatever is at `relative`, and forgets what was under it.
    fn remove(&mut self, root: &Path, relative: &Path) -> io::Result<()> {
        let path = root.join(relative);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => {
                make_writable(&path)?;
                fs::remove_dir_all(&path)?;
            }
            Ok(_) => fs::remove_file(&path)?,
            Err(_) => return Ok(()),
        }
        self.owners.retain(|path, _| !path.starts_with(relative));
        self.dir_modes.retain(|path, _| !path.starts_with(relative));
        Ok(())
    }
}

/// Every directory under `dir` writable by us, so it can be removed.
fn make_writable(dir: &Path) -> io::Result<()> {
    fs::set_permissions(dir, fs::Permissions::from_mode(0o755))?;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            make_writable(&entry.path())?;
        }
    }
    Ok(())
}

/// A member's path relative to the root; `None` for the root itself. An
/// absolute path is read from the root (`/etc` is `etc`); a `..` is an
/// error.
fn relative_path(name: &Path) -> io::Result<Option<PathBuf>> {
    let mut out = PathBuf::new();
    for part in name.components() {
        match part {
            Component::Normal(part) => out.push(part),
            Component::CurDir | Component::RootDir => {}
            _ => return Err(bad(name, "a `..` is not unpacked")),
        }
    }
    Ok((!out.as_os_str().is_empty()).then_some(out))
}

fn bad(name: &Path, why: &str) -> io::Error {
    io::Error::other(format!("{}: {why}", name.display()))
}

/// The layer's bytes, decompressed by what its first bytes say it is.
fn open(path: &Path) -> io::Result<Box<dyn Read>> {
    let mut magic = [0_u8; 4];
    let read = fs::File::open(path)?.read(&mut magic)?;
    let file = BufReader::new(fs::File::open(path)?);
    Ok(match &magic[..read] {
        [0x1f, 0x8b, ..] => Box::new(flate2::read::MultiGzDecoder::new(file)),
        [0x28, 0xb5, 0x2f, 0xfd] => Box::new(zstd::stream::read::Decoder::new(file)?),
        _ => Box::new(file),
    })
}
