//! Read-only planning of the package cache's eviction after a store, for
//! `gaia preview`: which entries [`evict_level`] would remove. Nothing here
//! changes a file. (The restore plan is shared: see
//! [`PackageCache::restore_plan`].)
use super::evict::eviction_walk;
use super::*;

/// A cache entry a store would evict to stay within the cache's size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CacheEviction {
    /// The entry's directory.
    pub(crate) path: PathBuf,
    pub(crate) size: u64,
}

impl PackageCache {
    /// The entries a store would evict to keep every level within its size
    /// (see `evict_level`): the same walk, assuming every removal succeeds.
    pub(crate) fn preview_evictions(&self) -> Vec<CacheEviction> {
        self.levels()
            .into_iter()
            .flat_map(|(_, level)| {
                eviction_walk(level, self.max_size, |_, _| true)
                    .into_iter()
                    .map(|(manifest, size)| CacheEviction {
                        path: manifest.with_extension(""),
                        size,
                    })
            })
            .collect()
    }
}
