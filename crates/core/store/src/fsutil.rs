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

/// Moves `from` to `to` with `rename`, which is [`rename`] except in tests.
/// When `to` is already there (the same content, committed by another step
/// or process), `from` is removed. True when `to` is now the content just
/// hashed: `from` itself, or a copy of it made because `from` is on another
/// file system (see [`move_into`]).
pub(crate) fn place(
    from: &Path,
    to: &Path,
    rename: fn(&Path, &Path) -> io::Result<()>,
) -> Result<bool> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io("create", parent, e))?;
    }
    if fs::symlink_metadata(to).is_ok() {
        return remove_tree(from).map(|()| false);
    }
    match move_into(from, to, rename) {
        Ok(()) => {
            if let Some(parent) = to.parent() {
                sync_dir(parent);
            }
            Ok(true)
        }
        Err(_) if fs::symlink_metadata(to).is_ok() => remove_tree(from).map(|()| false),
        Err(e) => Err(Error::io("move into the store", from, e)),
    }
}

/// `fs::rename` as a plain function, the shape [`place`] takes.
pub(crate) fn rename(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

/// The move itself: one rename, or when `from` is on another file system
/// (a RAM work directory, say), a copy then the rename of that copy into
/// place, so `to` is never a partial copy.
fn move_into(from: &Path, to: &Path, rename: fn(&Path, &Path) -> io::Result<()>) -> io::Result<()> {
    rename(from, to).or_else(|error| match error.kind() {
        // Moving a directory to another parent rewrites its `..`, which
        // needs write permission on the directory itself.
        io::ErrorKind::PermissionDenied if from.is_dir() => {
            add_owner_write(from)?;
            rename(from, to)
        }
        io::ErrorKind::CrossesDevices => copy_into(from, to),
        _ => Err(error),
    })
}

/// Copies `from` to a temporary name beside `to`, renames that into place,
/// then removes `from`. Modes, links and the tree's contents are copied as
/// [`copy_tree`] copies them, so the tree's digest is the same. A `from` that
/// cannot be removed is left for its scratch directory to remove.
fn copy_into(from: &Path, to: &Path) -> io::Result<()> {
    let name = to.file_name().unwrap_or_default().to_string_lossy();
    let temp = to.with_file_name(format!(".{name}.{}", unique_name()));
    let copied = if from.is_dir() {
        copy_tree(from, &temp).map_err(|e| io::Error::other(e.to_string()))
    } else {
        fs::copy(from, &temp).map(|_| ())
    };
    if let Err(error) = copied {
        let _ = remove_tree(&temp);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temp, to) {
        let _ = remove_tree(&temp);
        return Err(error);
    }
    sync_dir(to.parent().unwrap_or(to));
    let _ = remove_tree(from);
    Ok(())
}

/// The bytes the file or tree at `path` takes on disk: its blocks, for every
/// entry in it. The size of a step's work, as a RAM work directory would
/// hold it.
pub fn used_bytes(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    let own = metadata.blocks().saturating_mul(512);
    if !metadata.is_dir() {
        return own;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return own;
    };
    own.saturating_add(
        entries
            .flatten()
            .map(|entry| used_bytes(&entry.path()))
            .sum::<u64>(),
    )
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
