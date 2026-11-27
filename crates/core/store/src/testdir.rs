//! A test's temporary directory that is removed even when a test leaves a
//! read-only directory in it, as a build's output tree does: on drop, every
//! directory under it is made writable by its owner again first.
//! `tempfile::TempDir` alone leaves such a tree behind, silently.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// A temporary directory in `$TMPDIR`, or under a given directory.
#[derive(Debug)]
pub(crate) struct TestDir(Option<tempfile::TempDir>);

impl TestDir {
    pub(crate) fn new() -> TestDir {
        TestDir(Some(tempfile::tempdir().expect("tempdir")))
    }

    /// Under `parent` (`/dev/shm`), when one can be made there.
    pub(crate) fn new_in(parent: &str) -> Option<TestDir> {
        tempfile::Builder::new()
            .tempdir_in(parent)
            .ok()
            .map(|dir| TestDir(Some(dir)))
    }

    pub(crate) fn path(&self) -> &Path {
        self.0.as_ref().expect("the directory").path()
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        if let Some(dir) = self.0.take() {
            writable(dir.path());
            let _ = dir.close();
        }
    }
}

/// Gives every directory under `path` its owner's write permission back.
pub(crate) fn writable(path: &Path) {
    let mut stack: Vec<PathBuf> = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(meta) = std::fs::symlink_metadata(&dir) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let mode = meta.permissions().mode();
        if mode & 0o700 != 0o700 {
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode | 0o700));
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            stack.extend(entries.flatten().map(|entry| entry.path()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_only_tree_inside_is_removed_with_the_directory() {
        let dir = TestDir::new();
        let kept = dir.path().to_path_buf();
        let ro = dir.path().join("a/ro/inner");
        std::fs::create_dir_all(&ro).expect("dirs");
        std::fs::write(ro.join("f"), b"x").expect("write");
        for p in [ro.clone(), ro.parent().expect("ro").to_path_buf()] {
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o555)).expect("chmod");
        }
        drop(dir);
        assert!(!kept.exists(), "{} was left behind", kept.display());
    }
}
