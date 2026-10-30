//! Digests of many files at once, for the trees Gaia hashes on every build:
//! the target tree of a Buildroot image, package override trees, external
//! trees and package directories.
//!
//! Each digest is the lowercase sha256 of the file's bytes, exactly as
//! [`sha256_hex`] gives it. When a manifest directory is given, a digest the
//! manifest there records is reused while the file's size and mtime_ns still
//! match it: the trust model of `content_digests`. The remaining files are
//! hashed on a few threads, and their digests are recorded. Only the manifest's
//! own cache is consulted; the digests returned never depend on it.

use crate::content_digests::{
    Entry, modified_ns, read_entries, relative_key, sha256_hex, write_entries,
};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Upper bound on the threads one batch hashes on: the hashing is I/O and
/// cache bound, so more threads only add contention on a shared disk.
const MAX_HASH_THREADS: usize = 8;

/// Manifest key prefix for a file outside the manifest directory.
const ABSOLUTE_KEY_PREFIX: &str = "abs:";

/// The manifest key of `path`: its path relative to `base` when it lies under
/// it, otherwise its absolute path. `None` when the path cannot be a line of
/// the manifest (tabs or newlines in it) or is neither under `base` nor
/// absolute.
fn manifest_key(base: &Path, path: &Path) -> Option<String> {
    if let Some(relative) = relative_key(base, path) {
        return Some(relative);
    }
    if !path.is_absolute() {
        return None;
    }
    let text = path.to_str()?;
    (!text.contains(['\t', '\n'])).then(|| format!("{ABSOLUTE_KEY_PREFIX}{text}"))
}

fn key_path(base: &Path, key: &str) -> PathBuf {
    match key.strip_prefix(ABSOLUTE_KEY_PREFIX) {
        Some(absolute) => PathBuf::from(absolute),
        None => base.join(key),
    }
}

/// The size and mtime_ns a file's cache entry must match.
fn stat_of(path: &Path) -> Option<(u64, u128)> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    Some((metadata.len(), modified_ns(&metadata)?))
}

/// The sha256 digest of each path, in order. Files the manifest in
/// `manifest_dir` records with a matching size and mtime are not read again;
/// the others are hashed in parallel and recorded there. A file that cannot
/// be read yields its error.
pub(crate) fn digests_of_files(
    manifest_dir: Option<&Path>,
    paths: &[PathBuf],
) -> Vec<io::Result<String>> {
    let mut known: BTreeMap<String, Entry> = manifest_dir
        .map(|base| {
            read_entries(base)
                .into_iter()
                .map(|entry| (entry.relative.clone(), entry))
                .collect()
        })
        .unwrap_or_default();
    // Keys and stats are taken before any hashing: a file changed during the
    // batch then carries a stale stat, and the next lookup hashes it again.
    let keys = paths
        .iter()
        .map(|path| manifest_dir.and_then(|base| manifest_key(base, path)))
        .collect::<Vec<_>>();
    let stats = paths.iter().map(|path| stat_of(path)).collect::<Vec<_>>();

    let mut results: Vec<Option<io::Result<String>>> = (0..paths.len()).map(|_| None).collect();
    let mut misses = Vec::new();
    for index in 0..paths.len() {
        let hit = match (&keys[index], stats[index]) {
            (Some(key), Some((size, mtime_ns))) => known
                .get(key)
                .filter(|entry| entry.size == size && entry.mtime_ns == mtime_ns)
                .map(|entry| entry.digest.clone()),
            _ => None,
        };
        match hit {
            Some(digest) => results[index] = Some(Ok(digest)),
            None => misses.push(index),
        }
    }

    let hashed = parallel_map(&misses, |&index| sha256_hex(&paths[index]));
    let mut recorded = false;
    for (index, result) in misses.into_iter().zip(hashed) {
        if let (Ok(digest), Some(key), Some((size, mtime_ns))) =
            (&result, &keys[index], stats[index])
        {
            known.insert(
                key.clone(),
                Entry {
                    relative: key.clone(),
                    size,
                    mtime_ns,
                    digest: digest.clone(),
                },
            );
            recorded = true;
        }
        results[index] = Some(result);
    }

    if let Some(base) = manifest_dir.filter(|_| recorded) {
        // Entries of files that no longer exist are dropped, so the manifest
        // tracks the files that are there rather than every file ever seen.
        known.retain(|key, _| key_path(base, key).is_file());
        let entries = known.into_values().collect::<Vec<_>>();
        // A failed write only costs re-hashing next time.
        let _ = write_entries(base, &entries);
    }
    results
        .into_iter()
        .map(|result| result.unwrap_or_else(|| Err(io::Error::other("file was not hashed"))))
        .collect()
}

/// Runs `work` over `items` on up to `MAX_HASH_THREADS` threads, returning
/// the results in item order.
/// Runs `work` over `items` on up to `MAX_HASH_THREADS` threads, returning
/// the results in item order. A thread that panics yields no results for its
/// items, so the result can be shorter than `items`.
pub fn parallel_map<T: Sync, R: Send>(items: &[T], work: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism()
        .map_or(1, |count| count.get())
        .min(MAX_HASH_THREADS)
        .min(items.len());
    if threads <= 1 {
        return items.iter().map(work).collect();
    }
    let chunk = items.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let handles = items
            .chunks(chunk)
            .map(|part| scope.spawn(|| part.iter().map(&work).collect::<Vec<R>>()))
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect()
    })
}

/// The regular files under `root`, in no particular order. Directories are
/// followed; symbolic links are not (a link is not content of its own).
pub(crate) fn regular_files_under(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                files.push(path);
            }
        }
    }
    files
}

/// The digests of the regular files under a set of trees, computed up front
/// so a tree walk can look each one up instead of reading it.
pub struct TreeFileDigests {
    by_path: HashMap<PathBuf, Result<String, String>>,
}

impl TreeFileDigests {
    /// Digests every regular file under `roots` (see `digests_of_files`).
    pub fn for_roots(manifest_dir: Option<&Path>, roots: &[&Path]) -> Self {
        let paths = roots
            .iter()
            .flat_map(|root| regular_files_under(root))
            .collect::<Vec<_>>();
        let digests = digests_of_files(manifest_dir, &paths);
        let by_path = paths
            .into_iter()
            .zip(digests)
            .map(|(path, digest)| (path, digest.map_err(|error| error.to_string())))
            .collect();
        Self { by_path }
    }

    /// The sha256 of a file, or its read error text. Files outside the
    /// precomputed set are read directly.
    pub fn sha256(&self, path: &Path) -> Result<String, String> {
        match self.by_path.get(path) {
            Some(digest) => digest.clone(),
            None => sha256_hex(path).map_err(|error| error.to_string()),
        }
    }

    /// The sha256 of a file, or a placeholder naming the error (the value
    /// `file_sha256_or_placeholder` gives).
    pub fn sha256_or_placeholder(&self, path: &Path) -> String {
        self.sha256(path)
            .unwrap_or_else(|error| format!("sha256-error:{}:{error}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_digests::{CONTENT_DIGESTS_FILE, record_content_digests};
    use gaia_testkit::TempDir;

    #[test]
    fn batch_digests_match_the_one_file_digest() {
        let root = TempDir::new("gaia-digest-batch-match");
        let mut paths = Vec::new();
        for index in 0..40 {
            let path = root.join(format!("file-{index}.bin"));
            fs::write(&path, vec![index as u8; 1000 + index * 37]).expect("write");
            paths.push(path);
        }
        let batch = digests_of_files(None, &paths);
        for (path, digest) in paths.iter().zip(batch) {
            assert_eq!(digest.expect("digest"), sha256_hex(path).expect("single"));
        }
    }

    #[test]
    fn manifest_entries_are_trusted_until_size_or_mtime_changes() {
        let root = TempDir::new("gaia-digest-batch-cache");
        let base = root.join("base");
        fs::create_dir_all(base.join("target")).expect("dir");
        let file = base.join("target/a.txt");
        fs::write(&file, "first").expect("write");
        let outside = root.join("outside.txt");
        fs::write(&outside, "outside").expect("write");
        let paths = vec![file.clone(), outside.clone()];

        let first = digests_of_files(Some(&base), &paths);
        assert!(
            base.join(CONTENT_DIGESTS_FILE).is_file(),
            "manifest written"
        );
        let recorded = read_entries(&base);
        assert_eq!(recorded.len(), 2, "relative and absolute keys");

        // A same-size edit with its mtime restored is the documented blind
        // spot of the trust model: the cached digest is returned.
        let mtime = fs::metadata(&file)
            .expect("meta")
            .modified()
            .expect("mtime");
        fs::write(&file, "FIRST").expect("rewrite");
        let restored = fs::File::options().write(true).open(&file).expect("open");
        restored.set_modified(mtime).expect("restore mtime");
        let cached = digests_of_files(Some(&base), &paths);
        assert_eq!(
            cached[0].as_ref().expect("cached"),
            first[0].as_ref().expect("first")
        );

        // A changed size is seen.
        fs::write(&file, "much longer contents").expect("rewrite");
        let fresh = digests_of_files(Some(&base), &paths);
        assert_eq!(
            fresh[0].as_ref().expect("fresh"),
            &sha256_hex(&file).expect("hash")
        );
        assert_ne!(fresh[0].as_ref().ok(), first[0].as_ref().ok());
    }

    #[test]
    fn tree_digests_keep_the_manifest_of_other_callers() {
        let root = TempDir::new("gaia-digest-batch-shared");
        let base = root.join("base");
        fs::create_dir_all(&base).expect("dir");
        let collected = base.join("collect.txt");
        fs::write(&collected, "collected").expect("write");
        record_content_digests(&base, std::slice::from_ref(&collected)).expect("record");
        let tree = base.join("tree");
        fs::create_dir_all(tree.join("sub")).expect("tree");
        fs::write(tree.join("sub/x"), "x").expect("write");
        let digests = TreeFileDigests::for_roots(Some(&base), &[tree.as_path()]);
        assert_eq!(
            digests.sha256(&tree.join("sub/x")).expect("x"),
            sha256_hex(&tree.join("sub/x")).expect("hash")
        );
        assert!(
            read_entries(&base)
                .iter()
                .any(|entry| entry.relative == "collect.txt"),
            "entries of other callers survive"
        );
    }
}
