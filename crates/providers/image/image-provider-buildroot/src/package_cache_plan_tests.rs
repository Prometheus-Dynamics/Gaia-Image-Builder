//! The plans `gaia preview` shows are the ones the cache performs: the restore
//! plan is `restore_except`'s, the eviction list is `evict`'s.
use super::tests::{built_tree, cache_graph, temp, test_cache, tools_available};
use super::*;

#[test]
fn the_preview_restores_what_restore_except_restores() {
    if !tools_available() {
        return;
    }
    let root = temp("restore-plan");
    let cache = test_cache(&root, &[]);
    let first = root.join("first");
    built_tree(&first, b"app binary");
    let graph = cache_graph();
    let keys = BTreeMap::from([
        ("base".to_string(), Some("k-base".to_string())),
        ("app".to_string(), Some("k-app".to_string())),
    ]);
    let (stored, skipped) = cache.store(&first, &graph, &keys, &[]);
    assert_eq!(stored, ["app", "base"], "{skipped:?}");

    // Each case restores into its own empty tree, so the plans start alike.
    let cases = [
        (BTreeSet::new(), vec!["app", "base"]),
        (BTreeSet::from(["app".to_string()]), vec!["base"]),
    ];
    for (index, (excluded, expected)) in cases.into_iter().enumerate() {
        let second = root.join(format!("second-{index}"));
        fs::create_dir_all(&second).expect("second");
        let built = |name: &str| stamp_built(&second, &graph, name);
        let mut planned = cache.restore_plan(&second, &graph, &keys, &excluded, &built);
        planned.sort();
        assert_eq!(planned, expected);
        let restored = cache.restore_except(&second, &graph, &keys, &excluded);
        assert_eq!(restored, planned, "excluded {excluded:?}");
    }
    let _ = fs::remove_dir_all(root);
}

/// Every cache entry directory under the cache's levels.
fn cached_entries(cache: &PackageCache) -> BTreeSet<PathBuf> {
    let mut entries = BTreeSet::new();
    for (_, level) in cache.levels() {
        for package in fs::read_dir(level).into_iter().flatten().flatten() {
            for file in fs::read_dir(package.path()).into_iter().flatten().flatten() {
                let path = file.path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "json")
                {
                    entries.insert(path.with_extension(""));
                }
            }
        }
    }
    entries
}

#[test]
fn preview_evictions_are_the_entries_eviction_removes() {
    if !tools_available() {
        return;
    }
    let root = temp("evictions");
    let mut cache = test_cache(&root, &[]);
    let first = root.join("first");
    built_tree(&first, b"app binary");
    let graph = cache_graph();
    let keys = BTreeMap::from([
        ("base".to_string(), Some("k-base".to_string())),
        ("app".to_string(), Some("k-app".to_string())),
    ]);
    let (stored, skipped) = cache.store(&first, &graph, &keys, &[]);
    assert_eq!(stored, ["app", "base"], "{skipped:?}");
    let before = cached_entries(&cache);
    assert_eq!(before.len(), 2);

    // Over its size: every entry goes (the target is nine tenths of 1 byte).
    cache.max_size = 1;
    let previewed = cache
        .preview_evictions()
        .into_iter()
        .map(|eviction| eviction.path)
        .collect::<BTreeSet<_>>();
    assert_eq!(previewed, before);
    cache.evict();
    let after = cached_entries(&cache);
    let removed = before.difference(&after).cloned().collect::<BTreeSet<_>>();
    assert_eq!(removed, previewed);
    let _ = fs::remove_dir_all(root);
}
