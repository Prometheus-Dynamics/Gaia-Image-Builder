//! Eviction of the package cache's least recently used entries beyond its
//! size. [`eviction_order`] reads a level and [`eviction_walk`] applies the
//! stopping rule; the real eviction and `gaia preview` both use them.
use super::*;

/// The entries of `level`, and their total size: each entry's last use,
/// size and manifest path, least recently used first.
pub(super) fn eviction_order(level: &Path) -> (u64, Vec<(std::time::SystemTime, u64, PathBuf)>) {
    let mut entries = Vec::new();
    let mut total = 0u64;
    for package in fs::read_dir(level).into_iter().flatten().flatten() {
        for file in fs::read_dir(package.path()).into_iter().flatten().flatten() {
            let path = file.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let Some(manifest) = fs::read_to_string(&path)
                .ok()
                .and_then(|text| Manifest::parse(&text))
            else {
                continue;
            };
            let used = file
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            total += manifest.size;
            entries.push((used, manifest.size, path));
        }
    }
    entries.sort();
    (total, entries)
}

/// Walks `level`'s entries least recently used first, while the level is
/// over `max_size`, until it is down to nine tenths of it. Each entry is
/// handed to `remove` with its size; `remove` reports whether the removal
/// succeeded. A failed removal is passed over and does not count down the
/// level. Returns the entries removed, as (manifest, size).
pub(super) fn eviction_walk(
    level: &Path,
    max_size: u64,
    mut remove: impl FnMut(&Path, u64) -> bool,
) -> Vec<(PathBuf, u64)> {
    let (mut total, entries) = eviction_order(level);
    if total <= max_size {
        return Vec::new();
    }
    let target = max_size / 10 * 9;
    let mut removed = Vec::new();
    for (_, size, manifest) in entries {
        if total <= target {
            break;
        }
        if remove(&manifest, size) {
            total = total.saturating_sub(size);
            removed.push((manifest, size));
        }
    }
    removed
}

/// Removes a level's least recently used entries beyond `max_size`.
pub(super) fn evict_level(level: &Path, max_size: u64) {
    eviction_walk(level, max_size, |manifest, _| {
        let _ = gaia_process::discard(&manifest.with_extension(""));
        fs::remove_file(manifest).is_ok()
    });
}
