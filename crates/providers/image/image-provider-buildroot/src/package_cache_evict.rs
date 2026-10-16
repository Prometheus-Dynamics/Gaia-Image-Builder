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

/// Removes a level's least recently used entries beyond `max_size`. Returns
/// the entries removed, as (manifest, size).
pub(super) fn evict_level(level: &Path, max_size: u64) -> Vec<(PathBuf, u64)> {
    eviction_walk(level, max_size, |manifest, _| {
        let _ = gaia_process::discard(&manifest.with_extension(""));
        fs::remove_file(manifest).is_ok()
    })
}

const GIB: u64 = 1024 * 1024 * 1024;
/// The largest default `max_size` of a level.
pub(super) const DEFAULT_MAX_SIZE_CAP: u64 = 40 * GIB;
/// The smallest default `max_size` of a level.
const DEFAULT_MAX_SIZE_FLOOR: u64 = 2 * GIB;

/// The default `max_size` of a level when none is set: half the free space
/// on its filesystem, plus what the level already holds (so an existing
/// cache is not over budget merely for being there), capped at
/// [`DEFAULT_MAX_SIZE_CAP`] and never below [`DEFAULT_MAX_SIZE_FLOOR`].
/// Without a free-space reading (`None`) it is the cap.
pub(super) fn default_max_size(free: Option<u64>, current: u64) -> u64 {
    let Some(free) = free else {
        return DEFAULT_MAX_SIZE_CAP;
    };
    (free / 2)
        .saturating_add(current)
        .clamp(DEFAULT_MAX_SIZE_FLOOR, DEFAULT_MAX_SIZE_CAP)
}

impl PackageCache {
    /// The size a level is kept at: `max_size` when set, else its default.
    pub(crate) fn budget(&self, level: &Path) -> u64 {
        match self.max_size {
            Some(max_size) => max_size,
            None => self
                .default_budgets
                .iter()
                .find(|(dir, _)| dir == level)
                .map_or(DEFAULT_MAX_SIZE_CAP, |(_, budget)| *budget),
        }
    }

    /// Removes the least recently used entries beyond each level's budget,
    /// by their manifests (size, and last use as the manifest's modification
    /// time). Returns the number of entries removed and their total size.
    pub(crate) fn evict(&self) -> (usize, u64) {
        let mut count = 0;
        let mut bytes = 0;
        for (_, level) in self.levels() {
            let removed = evict_level(level, self.budget(level));
            count += removed.len();
            bytes += removed.iter().map(|(_, size)| size).sum::<u64>();
        }
        if count > 0 {
            tracing::info!(
                provider_domain = "image.buildroot",
                "package cache: evicted {count} least recently used package build(s), {bytes} bytes"
            );
        }
        (count, bytes)
    }
}
