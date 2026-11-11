//! Retention: removing what nothing kept refers to. It runs only when asked;
//! the store never deletes on its own.

use crate::{Digest, Error, Kind, ObjectRef, Result, StepKey, Store, entry, fsutil};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// What to keep. A lock names the step keys and objects one build used; the
/// caller passes what the latest N locks name, merged with [`Keep::extend`].
/// Every object a kept entry names is kept with it.
#[derive(Debug, Clone, Default)]
pub struct Keep {
    pub entries: BTreeSet<StepKey>,
    pub objects: BTreeSet<ObjectRef>,
}

impl Keep {
    pub fn extend(&mut self, other: &Keep) {
        self.entries.extend(other.entries.iter().copied());
        self.objects.extend(other.objects.iter().copied());
    }
}

/// What [`Store::retain`] removed, or would remove when not applied.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetainReport {
    pub entries_kept: usize,
    pub entries_removed: usize,
    pub objects_kept: usize,
    pub objects_removed: usize,
    /// Scratch directories and half-written entries of processes that are
    /// gone.
    pub leftovers_removed: usize,
    pub bytes_removed: u64,
}

impl Store {
    /// Removes every entry not in `keep`, every object neither `keep` nor a
    /// kept entry names, and what dead processes left in `scratch/`. With
    /// `apply` false, only reports what it would remove.
    ///
    /// Run it alone: a step committing meanwhile could lose an object
    /// between its commit and its entry.
    pub fn retain(&self, keep: &Keep, apply: bool) -> Result<RetainReport> {
        let mut report = RetainReport::default();
        let mut objects = keep.objects.clone();
        let remove = |path: &Path, report: &mut RetainReport| -> Result<()> {
            report.bytes_removed += disk_bytes(path);
            if apply {
                fsutil::remove_tree(path)?;
            }
            Ok(())
        };

        for (name, path) in children_two_deep(&self.root().join("entries"))? {
            let key = name
                .strip_suffix(".json")
                .and_then(Digest::parse)
                .map(StepKey);
            match key {
                Some(key) if keep.entries.contains(&key) => {
                    report.entries_kept += 1;
                    if let Some(kept) = entry::read(&path)? {
                        objects.extend(kept.outputs.into_values());
                    }
                }
                Some(_) => {
                    report.entries_removed += 1;
                    remove(&path, &mut report)?;
                }
                // A temporary name: an entry still being written, or one a
                // dead process left.
                None => {
                    if !fsutil::name_pid(&name).is_some_and(fsutil::process_alive) {
                        report.leftovers_removed += 1;
                        remove(&path, &mut report)?;
                    }
                }
            }
        }

        for kind in [Kind::File, Kind::Tree] {
            let dir = self.root().join("objects").join(kind.dir_name());
            for (name, path) in children_two_deep(&dir)? {
                let kept = Digest::parse(&name)
                    .is_some_and(|digest| objects.contains(&ObjectRef { kind, digest }));
                if kept {
                    report.objects_kept += 1;
                } else {
                    report.objects_removed += 1;
                    remove(&path, &mut report)?;
                }
            }
        }

        let scratch = self.root().join("scratch");
        for (name, path) in children(&scratch)? {
            if !fsutil::name_pid(&name).is_some_and(fsutil::process_alive) {
                report.leftovers_removed += 1;
                remove(&path, &mut report)?;
            }
        }
        Ok(report)
    }
}

fn children(dir: &Path) -> Result<Vec<(String, std::path::PathBuf)>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| Error::io("list", dir, e))? {
        let entry = entry.map_err(|e| Error::io("list", dir, e))?;
        out.push((
            entry.file_name().to_string_lossy().into_owned(),
            entry.path(),
        ));
    }
    out.sort();
    Ok(out)
}

/// The entries of each `<2 hex>` directory under `dir`.
fn children_two_deep(dir: &Path) -> Result<Vec<(String, std::path::PathBuf)>> {
    let mut out = Vec::new();
    for (_, shard) in children(dir)? {
        if shard.is_dir() {
            out.extend(children(&shard)?);
        }
    }
    Ok(out)
}

fn disk_bytes(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !metadata.is_dir() {
        return metadata.len();
    }
    fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| disk_bytes(&entry.path()))
                .sum()
        })
        .unwrap_or(0)
}
