//! File system helpers: unique names, atomic placement, copying and removing
//! trees that may hold read-only directories.

use crate::{Error, Result};
use std::fs;
use std::io;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// `<pid>-<counter>-<nanos>`: unique within the store across processes. The
/// pid comes first so [`crate::Store::retain`] can tell which scratch
/// directories belong to a process that is gone.
pub(crate) fn unique_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!(
        "{}-{}-{nanos}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// The pid a [`unique_name`] was made by.
pub(crate) fn name_pid(name: &str) -> Option<u32> {
    name.trim_start_matches('.').split('-').next()?.parse().ok()
}

pub(crate) fn process_alive(pid: u32) -> bool {
    pid == std::process::id() || Path::new("/proc").join(pid.to_string()).exists()
}

/// Moves `from` to `to` with one rename. When `to` is already there (the
/// same content, committed by another step or process), `from` is removed.
pub(crate) fn place(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io("create", parent, e))?;
    }
    if fs::symlink_metadata(to).is_ok() {
        return remove_tree(from);
    }
    let renamed = fs::rename(from, to).or_else(|error| {
        // Moving a directory to another parent rewrites its `..`, which
        // needs write permission on the directory itself.
        if error.kind() == io::ErrorKind::PermissionDenied && from.is_dir() {
            add_owner_write(from)?;
            fs::rename(from, to)
        } else {
            Err(error)
        }
    });
    match renamed {
        Ok(()) => {
            if let Some(parent) = to.parent() {
                sync_dir(parent);
            }
            Ok(())
        }
        Err(_) if fs::symlink_metadata(to).is_ok() => remove_tree(from),
        Err(e) => Err(Error::io("move into the store", from, e)),
    }
}

/// Removes a file or a tree. Read-only directories inside it are made
/// writable first, since entries cannot be removed from them otherwise.
pub(crate) fn remove_tree(path: &Path) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if !metadata.is_dir() {
        return fs::remove_file(path).map_err(|e| Error::io("remove", path, e));
    }
    if fs::remove_dir_all(path).is_ok() {
        return Ok(());
    }
    make_dirs_writable(path);
    fs::remove_dir_all(path).map_err(|e| Error::io("remove", path, e))
}

fn make_dirs_writable(dir: &Path) {
    let _ = add_owner_write(dir);
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                make_dirs_writable(&entry.path());
            }
        }
    }
}

fn add_owner_write(path: &Path) -> io::Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(permissions.mode() | 0o200);
    fs::set_permissions(path, permissions)
}

/// Copies the tree at `from` to `to` (which must not exist): directories and
/// regular files with their permission bits, symlinks as links. Anything
/// else is an error.
pub(crate) fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(from).map_err(|e| Error::io("stat", from, e))?;
    if !metadata.is_dir() {
        return Err(Error::BadOutput {
            output: from.to_path_buf(),
            reason: "not a directory".into(),
        });
    }
    fs::create_dir(to).map_err(|e| Error::io("create", to, e))?;
    for entry in fs::read_dir(from).map_err(|e| Error::io("list", from, e))? {
        let entry = entry.map_err(|e| Error::io("list", from, e))?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|e| Error::io("stat", &source, e))?;
        if kind.is_dir() {
            copy_tree(&source, &target)?;
        } else if kind.is_file() {
            fs::copy(&source, &target).map_err(|e| Error::io("copy", &source, e))?;
        } else if kind.is_symlink() {
            let link = fs::read_link(&source).map_err(|e| Error::io("read link", &source, e))?;
            symlink(&link, &target).map_err(|e| Error::io("link", &target, e))?;
        } else {
            return Err(Error::Unsupported(source));
        }
    }
    // The mode last, so a read-only directory is filled before it closes.
    fs::set_permissions(to, metadata.permissions()).map_err(|e| Error::io("chmod", to, e))
}

/// Syncs a directory so a rename into it survives a power cut. Best effort:
/// a failure leaves the rename done, only not yet on disk.
pub(crate) fn sync_dir(dir: &Path) {
    if let Ok(file) = fs::File::open(dir) {
        let _ = file.sync_all();
    }
}
