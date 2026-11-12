//! What a report reads: each owner's files and their sizes from the store,
//! and the locked sources each owner was built from.

use crate::masking::mask_url;
use crate::model::{ComponentRecord, FileRecord, SourceRecord};
use gaia_lock::{Entry, Lock};
use gaia_store::Store;
use gaia_tree::Layer;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The files of all layers, sorted by path. A path several layers claim
/// takes the owners of all of them, and the last layer's bytes and mode.
/// Components are the owners in layer order; each counts the files it is
/// the last claimant of, so they add up to the whole.
pub(crate) fn files(
    store: &Store,
    layers: &[Layer],
) -> Result<(Vec<FileRecord>, Vec<ComponentRecord>), String> {
    let mut by_path: BTreeMap<String, FileRecord> = BTreeMap::new();
    for layer in layers {
        let root = layer
            .tree
            .object
            .path_in(store)
            .ok_or_else(|| format!("{}: its tree is not in the store", layer.owner))?;
        let modes: BTreeMap<String, u32> = layer
            .tree
            .entries
            .iter()
            .map(|e| (e.path.trim_start_matches('/').to_string(), e.mode & 0o7777))
            .collect();
        let mut found = Vec::new();
        walk(&root, "", &mut found).map_err(|e| format!("{}: {e}", layer.owner))?;
        for item in found {
            let mode = modes.get(&item.path).copied().unwrap_or(item.mode);
            match by_path.get_mut(&item.path) {
                Some(file) => {
                    file.owners.push(layer.owner.clone());
                    file.kind = item.kind.to_string();
                    file.mode = format!("{mode:04o}");
                    file.bytes = item.bytes;
                }
                None => {
                    by_path.insert(
                        item.path.clone(),
                        FileRecord {
                            path: item.path,
                            owners: vec![layer.owner.clone()],
                            kind: item.kind.to_string(),
                            mode: format!("{mode:04o}"),
                            bytes: item.bytes,
                        },
                    );
                }
            }
        }
    }
    let files: Vec<FileRecord> = by_path.into_values().collect();
    let components = layers
        .iter()
        .map(|layer| {
            let mine = files
                .iter()
                .filter(|f| f.owners.last() == Some(&layer.owner));
            let (count, bytes) = mine.fold((0u64, 0u64), |(n, b), f| (n + 1, b + f.bytes));
            ComponentRecord {
                owner: layer.owner.clone(),
                files: count,
                bytes,
            }
        })
        .collect();
    Ok((files, components))
}

/// The lock's sources of each owner: every entry named `<owner>.<source>`.
/// URLs and references are masked.
pub(crate) fn sources(lock: &Lock, owners: &[&str]) -> Vec<SourceRecord> {
    let mut found = Vec::new();
    for (key, entry) in &lock.entries {
        let owner = owners.iter().find(|owner| {
            key.strip_prefix(**owner)
                .and_then(|rest| rest.strip_prefix('.'))
                .is_some()
        });
        let Some(owner) = owner else { continue };
        let source = key[owner.len() + 1..].to_string();
        let (kind, locator) = match entry {
            Entry::Git { url, .. } => ("git", mask_url(url)),
            Entry::Download { url, .. } => ("download", mask_url(url)),
            Entry::Image { reference, .. } => ("image", mask_url(reference)),
            Entry::Tree { path, .. } => ("tree", mask_url(path)),
        };
        found.push(SourceRecord {
            owner: owner.to_string(),
            source,
            kind: kind.to_string(),
            locator,
            resolved: entry.resolved().to_string(),
        });
    }
    found.sort_by(|a, b| (&a.owner, &a.source).cmp(&(&b.owner, &b.source)));
    found
}

/// One path of a stored tree.
struct Found {
    path: String,
    kind: &'static str,
    mode: u32,
    bytes: u64,
}

/// Every file and symlink under `root`, relative and `/`-separated. Symlinks
/// count 0 bytes; directories are not listed.
fn walk(root: &Path, prefix: &str, found: &mut Vec<Found>) -> io::Result<()> {
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(root)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name().into_string().map_err(|name| {
            io::Error::other(format!("{}: not UTF-8", PathBuf::from(name).display()))
        })?;
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let meta = fs::symlink_metadata(entry.path())?;
        let kind = meta.file_type();
        if kind.is_dir() {
            walk(&entry.path(), &path, found)?;
        } else if kind.is_symlink() {
            found.push(Found {
                path,
                kind: "symlink",
                mode: 0,
                bytes: 0,
            });
        } else if kind.is_file() {
            found.push(Found {
                path,
                kind: "file",
                mode: meta.permissions().mode() & 0o7777,
                bytes: meta.len(),
            });
        } else {
            return Err(io::Error::other(format!(
                "{path} is not a file, directory or symlink"
            )));
        }
    }
    Ok(())
}
