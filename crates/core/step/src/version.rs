//! A tool's version, read once per process.
//!
//! A node keys on what `tool --version` prints. That is the same for every run
//! of one binary, so each program is asked once for the life of the process.
//! Only the bytes are kept: a failed run is not, and the next call runs again.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock, PoisonError};

/// The bytes `read` gives for `program`: `read` runs on the first call for that
/// path in this process, and later calls get the same bytes.
pub fn once_per_process(
    program: &Path,
    read: impl FnOnce() -> std::io::Result<Vec<u8>>,
) -> std::io::Result<Vec<u8>> {
    static CACHE: OnceLock<Mutex<BTreeMap<std::path::PathBuf, Vec<u8>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    let lock = || cache.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(bytes) = lock().get(program) {
        return Ok(bytes.clone());
    }
    let bytes = read()?;
    lock().insert(program.to_path_buf(), bytes.clone());
    Ok(bytes)
}
