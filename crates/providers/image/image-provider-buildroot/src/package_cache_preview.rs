//! Read-only planning of the package cache's work, for `gaia preview`: which
//! packages [`PackageCache::restore_except`] would restore, and which entries
//! the eviction after a store would remove. Both mirror the functions they
//! preview (`restore_except`, `evict_level`) and must be kept in step with
//! them; nothing here changes a file.
use super::*;

/// A cache entry a store would evict to stay within the cache's size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CacheEviction {
    /// The entry's directory.
    pub(crate) path: PathBuf,
    pub(crate) size: u64,
}

impl PackageCache {
    /// The packages [`Self::restore_except`] would restore, given the built
    /// set `built` (the stamps of the tree as it would be). `output_dir` is
    /// the tree the entries must be pinned to.
    pub(crate) fn preview_restore(
        &self,
        output_dir: &Path,
        graph: &PackageGraph,
        keys: &BTreeMap<String, Option<String>>,
        excluded: &BTreeSet<String>,
        built: &dyn Fn(&str) -> bool,
    ) -> Vec<String> {
        let order = dependency_order(graph);
        let plan = |excluded: &BTreeSet<String>| {
            let mut ready = order
                .iter()
                .filter(|name| built(name))
                .cloned()
                .collect::<BTreeSet<_>>();
            let mut plan = Vec::new();
            for name in &order {
                if ready.contains(name) || excluded.contains(name) {
                    continue;
                }
                let Some(package) = graph.get(name) else {
                    continue;
                };
                let dependencies_ready = package
                    .dependencies
                    .iter()
                    .all(|dependency| ready.contains(dependency));
                let cached = keys
                    .get(name)
                    .and_then(Option::as_deref)
                    .and_then(|key| self.usable(output_dir, name, key))
                    .is_some();
                if dependencies_ready && cached {
                    ready.insert(name.clone());
                    plan.push(name.clone());
                }
            }
            plan
        };
        let mut plan_list = plan(excluded);
        // Same rule as `restore_except`: out-of-tree kernel modules need the
        // kernel build tree, which an entry does not hold.
        if plan_list.iter().any(|name| name == "linux") {
            let dependents_ready = graph.get("linux").is_some_and(|linux| {
                linux.reverse_dependencies.iter().all(|dependent| {
                    plan_list.iter().any(|name| name == dependent) || built(dependent)
                })
            });
            if !dependents_ready {
                let mut without_linux = excluded.clone();
                without_linux.insert("linux".to_string());
                plan_list = plan(&without_linux);
            }
        }
        // A restore that fails is built instead; a preview assumes it works.
        let mut restored = BTreeSet::new();
        for name in plan_list {
            let Some(package) = graph.get(&name) else {
                continue;
            };
            if !package
                .dependencies
                .iter()
                .all(|dependency| restored.contains(dependency) || built(dependency))
            {
                continue;
            }
            if !matches!(keys.get(&name), Some(Some(_))) {
                continue;
            }
            restored.insert(name);
        }
        restored.into_iter().collect()
    }

    /// The entries a store would evict to keep every level within its size
    /// (see `evict_level`).
    pub(crate) fn preview_evictions(&self) -> Vec<CacheEviction> {
        self.levels()
            .into_iter()
            .flat_map(|(_, level)| eviction_candidates(level, self.max_size))
            .collect()
    }
}

/// The entries [`evict_level`] would remove from `level`, least recently
/// used first. Nothing is removed.
fn eviction_candidates(level: &Path, max_size: u64) -> Vec<CacheEviction> {
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
    if total <= max_size {
        return Vec::new();
    }
    entries.sort();
    let target = max_size / 10 * 9;
    let mut evicted = Vec::new();
    for (_, size, manifest) in entries {
        if total <= target {
            break;
        }
        evicted.push(CacheEviction {
            path: manifest.with_extension(""),
            size,
        });
        total = total.saturating_sub(size);
    }
    evicted
}
