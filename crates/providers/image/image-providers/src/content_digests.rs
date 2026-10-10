//! Content digests Gaia records for the files it writes.
//!
//! Gaia keeps a `.gaia-content-digests` manifest in a base directory: a collect
//! dir for the image build, or the directory holding an assembly output or an
//! archive. Each line records one regular file under that base:
//!
//! `<relative path>\t<size>\t<mtime_ns>\t<sha256 hex>`
//!
//! A lookup trusts an entry only while the file's current size and mtime_ns
//! equal the recorded ones; otherwise the caller must hash the file. Trust is
//! therefore (size, mtime_ns): a change that keeps both (an edit followed by a
//! restored mtime, for example) is not seen. The manifest is written atomically
//! (temporary file, then rename) and is skipped by every collect-dir walk, so
//! writing it never changes a collect dir's digest.

use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Digest manifest kept in a base directory.
pub const CONTENT_DIGESTS_FILE: &str = ".gaia-content-digests";
/// Provider bookkeeping in a collect dir, rewritten on every run and so
/// excluded from content walks.
pub const COLLECT_STATE_FILE: &str = ".gaia-image-state.txt";
/// Suffix of temporary files written before a rename into place.
pub const TEMP_FILE_SUFFIX: &str = ".gaia.tmp";

const HASH_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    relative: String,
    size: u64,
    mtime_ns: u128,
    digest: String,
}

/// Whether a directory entry is provider bookkeeping or a temporary file, and
/// so is not part of a collect dir's content.
pub fn is_content_walk_excluded(name: &str) -> bool {
    name == COLLECT_STATE_FILE || name == CONTENT_DIGESTS_FILE || name.ends_with(TEMP_FILE_SUFFIX)
}

/// Regular files under `dir`, following symlinked files but not symlinked
/// directories (so a link cycle cannot stall the walk). Excluded names are
/// skipped at every depth.
pub fn collect_content_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_into(dir, &mut files);
    files
}

fn collect_into(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_content_walk_excluded(&name) {
            continue;
        }
        let is_real_dir = fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir());
        if is_real_dir {
            collect_into(&path, files);
        } else if fs::metadata(&path).is_ok_and(|meta| meta.is_file()) {
            files.push(path);
        }
    }
}

/// Lowercase hex sha256 of a file's contents.
pub fn sha256_hex(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// The digest recorded for `path` under manifest directory `base`, when the
/// file's current size and mtime match the entry. `None` means hash the file.
pub fn recorded_sha256(base: &Path, path: &Path) -> Option<String> {
    let relative = relative_key(base, path)?;
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let mtime_ns = modified_ns(&metadata)?;
    read_entries(base)
        .into_iter()
        .find(|entry| {
            entry.relative == relative && entry.size == metadata.len() && entry.mtime_ns == mtime_ns
        })
        .map(|entry| entry.digest)
}

/// Records the digests of `paths` (regular files under `base`) in the
/// manifest, hashing only those whose entry is missing or stale. Files outside
/// `base` and non-files are skipped.
pub fn record_content_digests(base: &Path, paths: &[PathBuf]) -> io::Result<()> {
    record_content_entries(
        base,
        paths.iter().map(|path| (path.clone(), None)).collect(),
    )
}

/// Like `record_content_digests`, but each path may carry a digest the caller
/// already computed from its current bytes, so it is not hashed again.
pub(crate) fn record_content_entries(
    base: &Path,
    items: Vec<(PathBuf, Option<String>)>,
) -> io::Result<()> {
    let mut entries = read_entries(base);
    let mut changed = false;
    for (path, known_digest) in items {
        let Some(relative) = relative_key(base, &path) else {
            continue;
        };
        // Metadata is taken before hashing: if the file changes in between,
        // the recorded mtime or size is stale and lookups will re-hash.
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let Some(mtime_ns) = modified_ns(&metadata) else {
            continue;
        };
        let size = metadata.len();
        let current = entries.iter().any(|entry| {
            entry.relative == relative && entry.size == size && entry.mtime_ns == mtime_ns
        });
        if current {
            continue;
        }
        let digest = match known_digest {
            Some(digest) => digest,
            None => sha256_hex(&path)?,
        };
        entries.retain(|entry| entry.relative != relative);
        entries.push(Entry {
            relative,
            size,
            mtime_ns,
            digest,
        });
        changed = true;
    }
    if changed {
        write_entries(base, &entries)?;
    }
    Ok(())
}

/// Records every regular file of a collect dir (the manifest itself excluded).
pub fn record_collect_dir_digests(dir: &Path) -> io::Result<()> {
    record_content_digests(dir, &collect_content_files(dir))
}

fn relative_key(base: &Path, path: &Path) -> Option<String> {
    let relative = path
        .strip_prefix(base)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    // Tabs and newlines would break the line format; such paths are simply
    // never recorded and always re-hashed.
    if relative.is_empty() || relative.contains(['\t', '\n']) {
        return None;
    }
    Some(relative)
}

fn modified_ns(metadata: &fs::Metadata) -> Option<u128> {
    metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_nanos())
}

fn read_entries(base: &Path) -> Vec<Entry> {
    let Ok(text) = fs::read_to_string(base.join(CONTENT_DIGESTS_FILE)) else {
        return Vec::new();
    };
    text.lines().filter_map(parse_entry).collect()
}

fn parse_entry(line: &str) -> Option<Entry> {
    let mut fields = line.split('\t');
    let relative = fields.next()?;
    let size = fields.next()?.parse().ok()?;
    let mtime_ns = fields.next()?.parse().ok()?;
    let digest = fields.next()?;
    if fields.next().is_some() || relative.is_empty() {
        return None;
    }
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(Entry {
        relative: relative.to_string(),
        size,
        mtime_ns,
        digest: digest.to_string(),
    })
}

fn write_entries(base: &Path, entries: &[Entry]) -> io::Result<()> {
    let manifest = base.join(CONTENT_DIGESTS_FILE);
    let temp = base.join(format!("{CONTENT_DIGESTS_FILE}{TEMP_FILE_SUFFIX}"));
    let mut text = String::new();
    for entry in entries {
        text.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            entry.relative, entry.size, entry.mtime_ns, entry.digest
        ));
    }
    let written = (|| -> io::Result<()> {
        let mut file = fs::File::create(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    fs::rename(&temp, &manifest).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}
