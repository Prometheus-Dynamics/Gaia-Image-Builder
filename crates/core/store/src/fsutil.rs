//! File system helpers: unique names, atomic placement, copying and removing
//! trees that may hold read-only directories.

use crate::{Error, Result};
use std::fs;
use std::io;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The superblock magic of btrfs, as `statfs` reports it in `f_type`.
const BTRFS_SUPER_MAGIC: i64 = 0x9123_683E;

/// The `btrfs` tool, the first of these paths that is a file is used. A fixed
/// list rather than `PATH`, so where a store is made does not depend on the
/// environment of the process that makes it.
const BTRFS_TOOLS: [&str; 4] = [
    "/usr/bin/btrfs",
    "/bin/btrfs",
    "/usr/sbin/btrfs",
    "/sbin/btrfs",
];

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
/// or process), `from` is left where it is: it is inside the scratch
/// directory that holds it, which removes it when the scratch is dropped.
/// True when `to` is now the content just hashed: `from` itself, or a copy
/// of it made because `from` is on another file system (see [`move_into`]).
pub(crate) fn place(
    from: &Path,
    to: &Path,
    rename: fn(&Path, &Path) -> io::Result<()>,
) -> Result<bool> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io("create", parent, e))?;
    }
    if fs::symlink_metadata(to).is_ok() {
        return Ok(false);
    }
    match move_into(from, to, rename) {
        Ok(()) => {
            if let Some(parent) = to.parent() {
                sync_dir(parent);
            }
            Ok(true)
        }
        Err(_) if fs::symlink_metadata(to).is_ok() => Ok(false),
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

/// Copies `from` to a temporary name beside `to`, then renames that into
/// place. Modes, links and the tree's contents are copied as [`copy_tree`]
/// copies them, so the tree's digest is the same. `from` is left for its
/// scratch directory to dispose of.
fn copy_into(from: &Path, to: &Path) -> io::Result<()> {
    let name = to.file_name().unwrap_or_default().to_string_lossy();
    let temp = to.with_file_name(format!(".{name}.{}", unique_name()));
    let copied = if from.is_dir() {
        copy_tree(from, &temp).map_err(|e| io::Error::other(e.to_string()))
    } else {
        copy_file(from, &temp)
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
            copy_file(&source, &target).map_err(|e| Error::io("copy", &source, e))?;
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

/// Whether `path` is on a btrfs file system. False when it cannot be asked.
pub(crate) fn on_btrfs(path: &Path) -> bool {
    rustix::fs::statfs(path).is_ok_and(|stat| stat.f_type == BTRFS_SUPER_MAGIC)
}

/// Whether `path` is a btrfs subvolume (its root directory has inode 256).
#[cfg(test)]
pub(crate) fn is_subvolume(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    const BTRFS_SUBVOL_INO: u64 = 256;
    on_btrfs(path) && fs::metadata(path).is_ok_and(|m| m.is_dir() && m.ino() == BTRFS_SUBVOL_INO)
}

/// Creates a btrfs subvolume at `root`, with the `btrfs` tool (`btrfs
/// subvolume create`), which makes it by the same ioctl and needs no
/// privilege. Fails when there is no tool, or on any other file system.
pub(crate) fn subvolume_create(root: &Path) -> io::Result<()> {
    let tool = BTRFS_TOOLS
        .iter()
        .find(|path| Path::new(path).is_file())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no btrfs tool"))?;
    let status = Command::new(tool)
        .args(["subvolume", "create"])
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{tool} subvolume create exited with {status}"
        )))
    }
}

/// Makes the directory `root` of a new store: a btrfs subvolume when its
/// parent is on btrfs, so the store has its own file system tree and locks
/// (see the module docs of `lib`), and a plain directory otherwise. An
/// existing directory is left as it is.
pub(crate) fn make_root(root: &Path) -> Result<()> {
    make_root_with(root, subvolume_create)
}

/// [`make_root`] with the subvolume creation given, so the fallback can be
/// tested on any file system.
pub(crate) fn make_root_with(
    root: &Path,
    create_subvolume: fn(&Path) -> io::Result<()>,
) -> Result<()> {
    if root.is_dir() {
        return Ok(());
    }
    let parent = match root.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    fs::create_dir_all(parent).map_err(|e| Error::io("create", parent, e))?;
    if on_btrfs(parent) && create_subvolume(root).is_ok() {
        return Ok(());
    }
    match fs::create_dir(root) {
        Ok(()) => Ok(()),
        // Another process made it between the check and here.
        Err(_) if root.is_dir() => Ok(()),
        Err(e) => Err(Error::io("create", root, e)),
    }
}

/// Moves `path` into the directory `trash`, under a fresh name: a rename,
/// which is instant on the same file system where removing the tree is not.
/// An empty directory is removed outright. A `path` that cannot be renamed is
/// removed instead. `trash` must be on the same file system as `path`, which
/// the store makes sure of by keeping it inside its own root.
pub(crate) fn bury(path: &Path, trash: &Path) -> Result<()> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if metadata.is_dir() && fs::remove_dir(path).is_ok() {
        return Ok(());
    }
    fs::create_dir_all(trash).map_err(|e| Error::io("create", trash, e))?;
    let target = trash.join(unique_name());
    let moved = rename(path, &target).or_else(|error| match error.kind() {
        io::ErrorKind::PermissionDenied if metadata.is_dir() => {
            add_owner_write(path)?;
            rename(path, &target)
        }
        _ => Err(error),
    });
    match moved {
        Ok(()) => Ok(()),
        Err(_) => remove_tree(path),
    }
}

/// Copies one regular file, as a reflink (a copy-on-write clone: metadata
/// only, on btrfs and XFS) where the file system and the two files allow it,
/// and as a byte copy otherwise. Either way the mode is set after the
/// bytes, so the setuid and setgid bits are kept.
///
/// Every file copy in Gaia is this one: `std::fs::copy`, which it stands in
/// for and whose paths it takes, is disallowed (`clippy.toml`).
pub fn copy_file(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    copy_one(from.as_ref(), to.as_ref())
}

fn copy_one(from: &Path, to: &Path) -> io::Result<()> {
    if clone_file(from, to).is_ok() {
        return Ok(());
    }
    // A failed clone may have left an empty file behind.
    let _ = fs::remove_file(to);
    plain_copy(from, to)
}

/// The bytes, then the mode. `fs::copy` sets the mode first, and the kernel
/// clears the setuid and setgid bits when a process without `CAP_FSETID`
/// writes the file: a copy where no reflink is possible lost them, and the
/// tree's digest changed with the file system it was copied on.
#[allow(clippy::disallowed_methods)] // The one place `fs::copy` is called.
fn plain_copy(from: &Path, to: &Path) -> io::Result<()> {
    fs::copy(from, to)?;
    fs::set_permissions(to, fs::metadata(from)?.permissions())
}

/// The reflink half of [`copy_file`]: `to` must not exist.
fn clone_file(from: &Path, to: &Path) -> io::Result<()> {
    let source = fs::File::open(from)?;
    let permissions = source.metadata()?.permissions();
    let target = fs::File::create_new(to)?;
    rustix::fs::ioctl_ficlone(&target, &source).map_err(io::Error::from)?;
    target.set_permissions(permissions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A directory on the file system the tests run on: the temp dir, which
    /// is btrfs where `TMPDIR` says so.
    fn base() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    /// A tree with a read-only directory, as a build leaves one.
    fn tree(dir: &Path) {
        fs::create_dir_all(dir.join("ro")).expect("dirs");
        fs::write(dir.join("ro/data"), b"data").expect("write");
        fs::set_permissions(dir.join("ro"), fs::Permissions::from_mode(0o555)).expect("chmod");
    }

    fn failing(_: &Path) -> io::Result<()> {
        Err(io::Error::other("refused"))
    }

    /// The setuid, setgid and sticky bits survive a copy that is not a
    /// reflink, as they do a reflink.
    /// The setuid, setgid and sticky bits survive each copy: the byte copy,
    /// and [`copy_file`], the one every node uses (a reflink where the temp
    /// dir's file system has them), onto a new name and over an old file.
    #[test]
    fn every_copy_keeps_the_special_bits() {
        let dir = base();
        type Copy = fn(&Path, &Path) -> io::Result<()>;
        let copies: [(&str, Copy); 2] = [
            ("plain_copy", plain_copy),
            ("copy_file", |from, to| copy_file(from, to)),
        ];
        for (name, copy) in copies {
            for mode in [0o4755, 0o2755, 0o1755, 0o6711] {
                let (from, to) = (dir.path().join("from"), dir.path().join("to"));
                fs::write(&from, b"data").expect("file");
                fs::set_permissions(&from, fs::Permissions::from_mode(mode)).expect("mode");
                for over in [false, true] {
                    if over {
                        fs::write(&to, b"old bytes").expect("old file");
                    }
                    if name == "plain_copy" && over {
                        fs::remove_file(&to).expect("plain_copy makes a new file");
                    }
                    copy(&from, &to).expect("copy");
                    let copied = fs::metadata(&to).expect("copy").permissions().mode() & 0o7777;
                    assert_eq!(copied, mode, "{name} {mode:o} over={over}");
                    assert_eq!(fs::read(&to).expect("bytes"), b"data", "{name}");
                    fs::remove_file(&to).expect("remove");
                }
                fs::remove_file(&from).expect("remove");
            }
        }
    }

    #[test]
    fn a_new_root_is_a_subvolume_exactly_where_the_parent_is_btrfs() {
        use std::os::unix::fs::MetadataExt;
        let base = base();
        let root = base.path().join("store");
        make_root(&root).expect("root");
        assert!(root.is_dir());
        // What btrfs is, asked directly rather than through `on_btrfs`.
        let parent_is_btrfs =
            rustix::fs::statfs(base.path()).is_ok_and(|stat| stat.f_type == BTRFS_SUPER_MAGIC);
        let root_inode = fs::metadata(&root).expect("stat").ino();
        assert_eq!(root_inode == 256, parent_is_btrfs);
        assert_eq!(is_subvolume(&root), parent_is_btrfs);
    }

    #[test]
    fn a_refused_subvolume_falls_back_to_a_plain_directory() {
        let base = base();
        let root = base.path().join("store");
        make_root_with(&root, failing).expect("the fallback");
        assert!(root.is_dir());
        assert!(!is_subvolume(&root), "the refused subvolume is not made");
    }

    #[test]
    fn an_existing_root_is_left_alone() {
        let base = base();
        let root = base.path().join("store");
        fs::create_dir(&root).expect("dir");
        fs::write(root.join("marker"), b"kept").expect("write");
        make_root_with(&root, failing).expect("no-op");
        assert_eq!(fs::read(root.join("marker")).expect("read"), b"kept");
    }

    #[test]
    fn a_root_under_a_missing_parent_gets_the_parent() {
        let base = base();
        let root = base.path().join("a/b/store");
        make_root_with(&root, failing).expect("root");
        assert!(root.is_dir());
    }

    #[test]
    fn a_reflink_or_copy_keeps_bytes_and_mode() {
        let base = base();
        let from = base.path().join("from");
        let to = base.path().join("to");
        fs::write(&from, b"bytes to keep").expect("write");
        fs::set_permissions(&from, fs::Permissions::from_mode(0o750)).expect("chmod");
        copy_file(&from, &to).expect("copy");
        assert_eq!(fs::read(&to).expect("read"), b"bytes to keep");
        let mode = fs::metadata(&to).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o750);
    }

    #[test]
    fn a_clone_works_on_btrfs() {
        let base = base();
        let from = base.path().join("from");
        fs::write(&from, b"x").expect("write");
        let result = clone_file(&from, &base.path().join("to"));
        if on_btrfs(base.path()) {
            assert!(result.is_ok(), "btrfs clones within one file system");
        }
    }

    #[test]
    fn a_copy_across_file_systems_falls_back_to_bytes() {
        // A clone cannot cross file systems, and /dev/shm is tmpfs.
        let Ok(shm) = tempfile::Builder::new().tempdir_in("/dev/shm") else {
            return;
        };
        let base = base();
        let from = shm.path().join("from");
        fs::write(&from, b"across the device").expect("write");
        if !on_btrfs(shm.path()) {
            let refused = clone_file(&from, &base.path().join("clone"));
            assert!(refused.is_err() || on_btrfs(base.path()));
        }
        let to = base.path().join("to");
        copy_file(&from, &to).expect("copy");
        assert_eq!(fs::read(&to).expect("read"), b"across the device");
    }

    #[test]
    fn a_tree_is_buried_in_the_trash_not_removed() {
        let base = base();
        let trash = base.path().join("trash");
        let victim = base.path().join("victim");
        tree(&victim);
        bury(&victim, &trash).expect("bury");
        assert!(!victim.exists());
        let kept: Vec<_> = fs::read_dir(&trash).expect("trash").flatten().collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(
            fs::read(kept[0].path().join("ro/data")).expect("read"),
            b"data"
        );
    }

    #[test]
    fn an_empty_directory_is_removed_outright() {
        let base = base();
        let trash = base.path().join("trash");
        let empty = base.path().join("empty");
        fs::create_dir(&empty).expect("dir");
        bury(&empty, &trash).expect("bury");
        assert!(!empty.exists());
        assert!(!trash.exists(), "nothing was moved to the trash");
    }

    #[test]
    fn a_missing_path_is_nothing_to_bury() {
        let base = base();
        bury(&base.path().join("nothing"), &base.path().join("trash")).expect("nothing");
    }
}
