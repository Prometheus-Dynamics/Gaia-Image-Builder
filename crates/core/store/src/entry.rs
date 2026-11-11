//! Step entries: a step key and the outputs that step made.

use crate::{Digest, Error, ObjectRef, Result, fsutil};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

/// A step's key: a digest of its node kind, node version, config and its
/// inputs' digests. The step kit computes it; the store only files by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StepKey(pub Digest);

/// The outputs one step made, by output name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepEntry {
    pub outputs: BTreeMap<String, ObjectRef>,
}

pub(crate) fn read(path: &Path) -> Result<Option<StepEntry>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| Error::BadEntry {
                path: path.to_path_buf(),
                reason: e.to_string(),
            }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::io("read", path, e)),
    }
}

/// Writes the entry to a temporary name, syncs it, and renames it into
/// place, so a reader sees no entry or the whole entry.
pub(crate) fn write(path: &Path, entry: &StepEntry) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir).map_err(|e| Error::io("create", dir, e))?;
    let temporary = dir.join(format!(".{}", fsutil::unique_name()));
    let bytes = serde_json::to_vec_pretty(entry).map_err(|e| Error::BadEntry {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;
    let written = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&temporary);
        return Err(Error::io("write", path, e));
    }
    fsutil::sync_dir(dir);
    Ok(())
}
