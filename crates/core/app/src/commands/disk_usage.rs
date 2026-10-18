//! du-style disk usage of a directory tree: allocated blocks of every file
//! and directory, a file with several hard links counted once (by device
//! and inode), symlinks counted as themselves and never followed. The tree
//! is walked by a few threads sharing one queue of directories.

use std::collections::HashMap;
use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};

/// Totals of a tree, or of several merged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DiskUsage {
    /// Allocated on disk (`du`).
    pub bytes: u64,
    /// The sum of file lengths (`du --apparent-size`).
    pub apparent_bytes: u64,
    /// Files, symlinks and other non-directories, hard links once.
    pub files: u64,
    pub dirs: u64,
}

/// A tree's usage, with its multiply linked files kept apart so usages of
/// several trees can be merged without counting a shared inode twice.
#[derive(Debug, Clone, Default)]
pub(crate) struct TreeUsage {
    /// Directories and files with a single link.
    single: DiskUsage,
    /// `(device, inode)` of files with more than one link, with their
    /// allocated and apparent sizes.
    linked: HashMap<(u64, u64), (u64, u64)>,
}

impl TreeUsage {
    pub(crate) fn total(&self) -> DiskUsage {
        let mut total = self.single;
        for (bytes, apparent) in self.linked.values() {
            total.bytes += bytes;
            total.apparent_bytes += apparent;
            total.files += 1;
        }
        total
    }

    pub(crate) fn merge(&mut self, other: &TreeUsage) {
        self.single.bytes += other.single.bytes;
        self.single.apparent_bytes += other.single.apparent_bytes;
        self.single.files += other.single.files;
        self.single.dirs += other.single.dirs;
        self.linked
            .extend(other.linked.iter().map(|(key, value)| (*key, *value)));
    }

    fn add(&mut self, metadata: &Metadata) {
        let (bytes, apparent) = (allocated(metadata), metadata.len());
        if metadata.is_dir() {
            self.single.dirs += 1;
        } else if let Some(key) = shared_inode(metadata) {
            self.linked.insert(key, (bytes, apparent));
            return;
        } else {
            self.single.files += 1;
        }
        self.single.bytes += bytes;
        self.single.apparent_bytes += apparent;
    }
}

#[cfg(unix)]
fn allocated(metadata: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.blocks() * 512
}

#[cfg(not(unix))]
fn allocated(metadata: &Metadata) -> u64 {
    metadata.len()
}

#[cfg(unix)]
fn shared_inode(metadata: &Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    (metadata.nlink() > 1).then(|| (metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn shared_inode(_metadata: &Metadata) -> Option<(u64, u64)> {
    None
}

/// The directories still to read, and how many workers are reading one.
struct Queue {
    dirs: Vec<PathBuf>,
    busy: usize,
}

/// The usage of the tree at `root`, `None` when nothing is there. A root
/// that is a symlink counts as the link.
pub(crate) fn tree_usage(root: &Path) -> Option<TreeUsage> {
    let metadata = fs::symlink_metadata(root).ok()?;
    let mut usage = TreeUsage::default();
    usage.add(&metadata);
    if !metadata.is_dir() {
        return Some(usage);
    }
    let workers = std::thread::available_parallelism()
        .map_or(4, usize::from)
        .clamp(1, 8);
    let queue = Mutex::new(Queue {
        dirs: vec![root.to_path_buf()],
        busy: 0,
    });
    let ready = Condvar::new();
    let parts = std::thread::scope(|scope| {
        let handles = (0..workers)
            .map(|_| scope.spawn(|| walk(&queue, &ready)))
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .filter_map(|handle| handle.join().ok())
            .collect::<Vec<_>>()
    });
    for part in &parts {
        usage.merge(part);
    }
    Some(usage)
}

/// One worker: reads directories from the queue until it is empty and no
/// other worker can add to it.
fn walk(queue: &Mutex<Queue>, ready: &Condvar) -> TreeUsage {
    let mut usage = TreeUsage::default();
    loop {
        let dir = {
            let Ok(mut state) = queue.lock() else {
                return usage;
            };
            loop {
                if let Some(dir) = state.dirs.pop() {
                    state.busy += 1;
                    break dir;
                }
                if state.busy == 0 {
                    ready.notify_all();
                    return usage;
                }
                state = match ready.wait(state) {
                    Ok(state) => state,
                    Err(_) => return usage,
                };
            }
        };
        let mut subdirs = Vec::new();
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            // Not followed: a symlink's metadata is the link's.
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            usage.add(&metadata);
            if metadata.is_dir() {
                subdirs.push(entry.path());
            }
        }
        let Ok(mut state) = queue.lock() else {
            return usage;
        };
        state.busy -= 1;
        state.dirs.extend(subdirs);
        ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_testkit::TempDir;

    #[test]
    fn hard_links_count_once_and_symlinks_are_not_followed() {
        let scratch = TempDir::new("gaia-disk-usage");
        let root = scratch.join("tree");
        let outside = scratch.join("outside");
        fs::create_dir_all(root.join("a/b/c")).expect("dirs");
        fs::create_dir_all(&outside).expect("outside");
        fs::write(root.join("a/one"), vec![1u8; 10_000]).expect("one");
        fs::write(root.join("a/b/c/two"), vec![2u8; 5_000]).expect("two");
        fs::hard_link(root.join("a/one"), root.join("a/b/one-again")).expect("link");
        fs::write(outside.join("big"), vec![3u8; 100_000]).expect("big");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("to-outside")).expect("symlink");

        let usage = tree_usage(&root).expect("usage").total();
        assert_eq!(usage.dirs, 4);
        // one (once), two, and the symlink itself.
        assert_eq!(usage.files, if cfg!(unix) { 3 } else { 2 });
        assert!(usage.apparent_bytes >= 15_000, "{usage:?}");
        assert!(usage.apparent_bytes < 100_000, "{usage:?}");

        // Merging trees that share a hard-linked inode counts it once: `b`
        // has two and one-again, `a` has one (the same inode) and two.
        let mut merged = tree_usage(&root.join("a/b")).expect("b");
        assert_eq!(merged.total().files, 2);
        merged.merge(&tree_usage(&root.join("a")).expect("a"));
        assert_eq!(merged.total().files, if cfg!(unix) { 3 } else { 5 });
        assert_eq!(tree_usage(&scratch.join("missing")).map(|_| ()), None);
    }
}
