//! `gaia lock`: resolves every source a product's descriptions name and
//! writes the product's lock. Nothing else writes the lock.
//!
//! - A source already locked for what its description names keeps its
//!   entry: running it again moves nothing.
//! - A source new to the lock, or locked for another URL, ref, reference or
//!   path, is resolved.
//! - `--update <source>` resolves that one again, whatever the lock holds.
//! - A directory is hashed every time; its entry changes only when its
//!   contents did. Its path is recorded relative to the product.
//! - Entries no description uses are dropped.
//!
//! A git ref resolves to its commit (an annotated tag to the commit it
//! tags), a download to the digest of its bytes, an image tag to its
//! manifest digest, asked anonymously.

pub mod command;
mod resolve;

pub use resolve::{Tools, relative, resolve};

use gaia_lock::{Entry, Lock, Want};
use std::path::{Path, PathBuf};

/// What `gaia lock` was asked.
#[derive(Debug, Clone)]
pub struct Options {
    pub product: PathBuf,
    /// The lock; `gaia.lock` beside the product when not given.
    pub lock: Option<PathBuf>,
    /// Sources to resolve again.
    pub update: Vec<String>,
    pub tools: Tools,
    /// Where downloads and manifests are fetched to be hashed.
    pub scratch: PathBuf,
}

/// One entry that changed: none before means added, none after removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub name: String,
    pub before: Option<Entry>,
    pub after: Option<Entry>,
}

/// The lock's path for `options`.
pub fn lock_path(options: &Options) -> PathBuf {
    options
        .lock
        .clone()
        .unwrap_or_else(|| product_dir(&options.product).join("gaia.lock"))
}

/// The directory of the product file: `.` for a bare file name.
fn product_dir(product: &Path) -> &Path {
    product
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// What the descriptions ask for, a directory's path relative to the
/// product.
pub fn wants(product: &Path) -> Result<Vec<(String, Want)>, String> {
    let loaded = gaia_describe::Product::load(product).map_err(|e| e.to_string())?;
    let base = product_dir(product);
    Ok(loaded
        .wants()
        .into_iter()
        .map(|(name, want)| match want {
            Want::Tree { path } => (
                name,
                Want::Tree {
                    path: relative(Path::new(&path), base),
                },
            ),
            other => (name, other),
        })
        .collect())
}

/// Resolves what needs it and returns the new lock and what changed. Writes
/// nothing.
pub fn lock(options: &Options) -> Result<(Lock, Vec<Change>), String> {
    let base = product_dir(&options.product);
    let wants = wants(&options.product)?;
    for name in &options.update {
        if !wants.iter().any(|(n, _)| n == name) {
            let names: Vec<&str> = wants.iter().map(|(n, _)| n.as_str()).collect();
            return Err(format!(
                "no source `{name}`; the descriptions name: {}",
                names.join(", ")
            ));
        }
    }
    let path = lock_path(options);
    let old = if path.exists() {
        Lock::read(&path).map_err(|e| e.to_string())?
    } else {
        Lock::default()
    };
    let mut new = Lock::default();
    let mut errors = Vec::new();
    for (name, want) in &wants {
        let keep = old
            .get(name)
            .filter(|entry| !options.update.contains(name) && locked_for(entry, want))
            .filter(|_| !matches!(want, Want::Tree { .. }));
        let entry = match keep {
            Some(entry) => entry.clone(),
            None => {
                // A tree's path is relative to the product; hash it there.
                let want = match want {
                    Want::Tree { path } => Want::Tree {
                        path: base.join(path).display().to_string(),
                    },
                    other => other.clone(),
                };
                match resolve(&want, base, &options.tools, &options.scratch) {
                    Ok(entry) => entry,
                    Err(e) => {
                        errors.push(format!("{name}: {e}"));
                        continue;
                    }
                }
            }
        };
        new.insert(name, entry);
    }
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    let mut changes = Vec::new();
    let names: std::collections::BTreeSet<&String> =
        old.entries.keys().chain(new.entries.keys()).collect();
    for name in names {
        let (before, after) = (old.get(name), new.get(name));
        if before != after {
            changes.push(Change {
                name: name.clone(),
                before: before.cloned(),
                after: after.cloned(),
            });
        }
    }
    Ok((new, changes))
}

/// Whether `entry` was recorded for `want`.
fn locked_for(entry: &Entry, want: &Want) -> bool {
    let redact = gaia_lock::redact;
    match (want, entry) {
        (
            Want::Git { url, reference },
            Entry::Git {
                url: u,
                reference: r,
                ..
            },
        ) => redact(url) == *u && reference == r,
        (Want::Download { url }, Entry::Download { url: u, .. }) => redact(url) == *u,
        (Want::Image { reference }, Entry::Image { reference: r, .. }) => reference == r,
        (Want::Tree { path }, Entry::Tree { path: p, .. }) => path == p,
        _ => false,
    }
}

/// `changes` as lines: `+ name kind resolved`, `~ name old -> new`,
/// `- name`.
pub fn describe(changes: &[Change]) -> Vec<String> {
    changes
        .iter()
        .map(|change| match (&change.before, &change.after) {
            (None, Some(after)) => {
                format!("+ {}  {}  {}", change.name, after.kind(), after.resolved())
            }
            (Some(before), Some(after)) => format!(
                "~ {}  {} -> {}",
                change.name,
                before.resolved(),
                after.resolved()
            ),
            (Some(_), None) => format!("- {}", change.name),
            (None, None) => String::new(),
        })
        .collect()
}
