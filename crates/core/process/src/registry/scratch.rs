//! Files that exist for one step: curl configs (which hold credentials), the
//! header dump, and the bytes a signing tool reads. They are created 0600 under
//! a name no other process uses, and removed when dropped.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::{RegistryError, io_error};

static NEXT_NAME: AtomicU64 = AtomicU64::new(0);

/// A file removed when dropped.
pub(super) struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// Creates a new file in `dir` holding `contents`. `create_new` refuses an
    /// existing name, so the file is never a stale or foreign one.
    pub(super) fn create(dir: &Path, tag: &str, contents: &[u8]) -> Result<Self, RegistryError> {
        let unique = NEXT_NAME.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(
            ".gaia-registry-{}-{unique}-{tag}",
            std::process::id()
        ));
        let scratch = Self { path };
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&scratch.path)
            .map_err(|error| io_error(&scratch.path.display().to_string(), error))?;
        file.write_all(contents)
            .map_err(|error| io_error(&scratch.path.display().to_string(), error))?;
        Ok(scratch)
    }

    /// A path a tool will write; the file is removed when this is dropped.
    pub(super) fn named(path: PathBuf) -> Self {
        Self { path }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
