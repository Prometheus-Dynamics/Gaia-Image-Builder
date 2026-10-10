//! `--rebuild-package` against the package cache: the named package is
//! dircleaned and then left out of the restore, so the make builds it again.
use super::tests::{built_tree, cache_graph, temp, test_cache, tools_available};
use super::*;

#[test]
fn a_rebuild_package_is_dircleaned_and_not_restored_from_the_cache() {
    if !tools_available() {
        return;
    }
    let root = temp("rebuild-package");
    let cache = test_cache(&root, &[]);
    let first = root.join("first");
    built_tree(&first, b"app binary");
    let graph = cache_graph();
    let keys = BTreeMap::from([
        ("base".to_string(), Some("k-base".to_string())),
        ("app".to_string(), Some("k-app".to_string())),
    ]);
    let (stored, _) = cache.store(&first, &graph, &keys, &[]);
    assert_eq!(stored, ["app", "base"]);

    // `--rebuild-package app`: its build and per-package directories go,
    // its dependency's stay.
    let requested = BTreeSet::from(["app".to_string()]);
    let clean = crate::requested_rebuilds::with_requested_rebuilds(
        crate::clean_plan::CleanPlan::Nothing,
        &requested,
    );
    let crate::clean_plan::CleanPlan::Packages(rebuild) = &clean else {
        panic!("a package plan: {clean:?}");
    };
    crate::clean_plan::apply_package_rebuild(&first, rebuild, Some(&graph), &graph)
        .expect("dirclean");
    assert!(!first.join("build/app-1").exists());
    assert!(!first.join("per-package/app").exists());
    assert!(first.join("per-package/base").is_dir());

    // Restored into another tree: the requested package is left to make.
    let second = root.join("second");
    fs::create_dir_all(&second).expect("second");
    let excluded = crate::requested_rebuilds::excluded_from_restore(&BTreeSet::new(), &requested);
    assert_eq!(
        cache.restore_except(&second, &graph, &keys, &excluded),
        ["base"]
    );
    assert!(!second.join("per-package/app").exists());
    let _ = fs::remove_dir_all(root);
}
