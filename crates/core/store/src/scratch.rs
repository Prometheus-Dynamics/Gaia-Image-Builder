//! Scratch directories: where a step writes before its outputs are
//! committed.

use crate::{Error, Kind, ObjectRef, Result, StepEntry, StepKey, Store, fsutil};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// One step's output directory. Committing moves the named outputs into the
/// store; dropping it without a commit removes everything in it.
#[derive(Debug)]
pub struct Scratch<'s> {
    store: &'s Store,
    dir: PathBuf,
}

impl<'s> Scratch<'s> {
    pub(crate) fn new(store: &'s Store) -> Result<Self> {
        let dir = store.root().join("scratch").join(fsutil::unique_name());
        fs::create_dir(&dir).map_err(|e| Error::io("create", &dir, e))?;
        Ok(Self { store, dir })
    }

    /// The directory the step writes into.
    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// Moves each named output (a path relative to the scratch directory, a
    /// regular file or a directory) into the store, and returns their
    /// references. The scratch directory is removed either way.
    pub fn commit(self, outputs: &[(&str, &Path)]) -> Result<BTreeMap<String, ObjectRef>> {
        let mut found = Vec::with_capacity(outputs.len());
        for (name, relative) in outputs {
            let path = self.output_path(relative)?;
            let overlapping = found.iter().find(|(_, other, _): &&(&str, PathBuf, Kind)| {
                other.starts_with(&path) || path.starts_with(other)
            });
            if let Some((other, _, _)) = overlapping {
                return Err(Error::BadOutput {
                    output: relative.to_path_buf(),
                    reason: format!("overlaps output {other}"),
                });
            }
            let metadata = fs::symlink_metadata(&path).map_err(|e| Error::BadOutput {
                output: relative.to_path_buf(),
                reason: e.to_string(),
            })?;
            let kind = if metadata.is_file() {
                Kind::File
            } else if metadata.is_dir() {
                Kind::Tree
            } else {
                return Err(Error::BadOutput {
                    output: relative.to_path_buf(),
                    reason: "not a regular file or a directory".into(),
                });
            };
            found.push((*name, path, kind));
        }
        let mut refs = BTreeMap::new();
        for (name, path, kind) in found {
            let object = ObjectRef {
                kind,
                digest: kind.digest(&path)?,
            };
            fsutil::place(&path, &self.store.path(&object))?;
            refs.insert(name.to_string(), object);
        }
        Ok(refs)
    }

    /// Commits the outputs, then records them as `key`'s entry: the entry is
    /// written only once every output is in the store.
    pub fn commit_step(self, key: &StepKey, outputs: &[(&str, &Path)]) -> Result<StepEntry> {
        let store = self.store;
        let entry = StepEntry {
            outputs: self.commit(outputs)?,
        };
        store.record(key, &entry)?;
        Ok(entry)
    }

    pub(crate) fn commit_one(self, relative: &Path) -> Result<ObjectRef> {
        let refs = self.commit(&[("out", relative)])?;
        refs.into_values().next().ok_or_else(|| Error::BadOutput {
            output: relative.to_path_buf(),
            reason: "nothing committed".into(),
        })
    }

    /// Removes the scratch directory and everything in it.
    pub fn discard(self) {}

    fn output_path(&self, relative: &Path) -> Result<PathBuf> {
        let plain = relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
        if !plain || relative.as_os_str().is_empty() {
            return Err(Error::BadOutput {
                output: relative.to_path_buf(),
                reason: "must be a plain path inside the step's output directory".into(),
            });
        }
        Ok(self.dir.join(relative))
    }
}

impl Drop for Scratch<'_> {
    fn drop(&mut self) {
        let _ = fsutil::remove_tree(&self.dir);
    }
}
