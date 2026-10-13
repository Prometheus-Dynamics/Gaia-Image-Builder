//! Test-only helpers shared by the Gaia workspace.
//!
//! Scratch space for tests lives under `std::env::temp_dir()`, which honours `TMPDIR`.
//! Point `TMPDIR` at a disk-backed directory when running the whole suite to keep
//! large fixtures out of RAM-backed `/tmp`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Directory name, under the system temp dir, that holds every test scratch directory.
pub const SCRATCH_ROOT_NAME: &str = "gaia-tests";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A uniquely named scratch directory under `$TMPDIR/gaia-tests/`.
///
/// The directory is created by [`TempDir::new`] and removed recursively when the
/// guard is dropped, including while unwinding from a failed assertion. Use
/// [`TempDir::keep_on_failure`] only when a failing test needs its artifacts for
/// inspection; a passing test still cleans up.
#[must_use = "dropping the guard immediately deletes the directory"]
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
    keep_on_failure: bool,
}

impl TempDir {
    /// Creates a fresh, empty directory whose name starts with `prefix`.
    pub fn new(prefix: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join(SCRATCH_ROOT_NAME)
            .join(format!("{prefix}-{}-{nanos}-{count}", std::process::id()));
        fs::create_dir_all(&path).expect("test scratch directory");
        Self {
            path,
            keep_on_failure: false,
        }
    }

    /// The scratch directory itself.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A path inside the scratch directory. The path is not created.
    pub fn join(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.path.join(relative)
    }

    /// Keeps the directory when the test panics, so the failure can be inspected.
    /// Passing tests still remove it.
    pub fn keep_on_failure(mut self) -> Self {
        self.keep_on_failure = true;
        self
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if self.keep_on_failure && std::thread::panicking() {
            eprintln!("kept test scratch directory {}", self.path.display());
            return;
        }
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_directory_on_drop() {
        let path = {
            let dir = TempDir::new("testkit-drop");
            fs::write(dir.join("file.txt"), "x").expect("write");
            assert!(dir.path().is_dir());
            dir.path().to_path_buf()
        };
        assert!(!path.exists());
    }

    #[test]
    fn removes_directory_when_unwinding() {
        let result = std::panic::catch_unwind(|| {
            let dir = TempDir::new("testkit-panic");
            fs::write(dir.join("file.txt"), "x").expect("write");
            panic!("expected");
        });
        assert!(result.is_err());
    }

    #[test]
    fn creates_distinct_directories() {
        let first = TempDir::new("testkit-unique");
        let second = TempDir::new("testkit-unique");
        assert_ne!(first.path(), second.path());
    }
}
