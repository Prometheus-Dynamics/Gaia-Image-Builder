//! The package cache's size budget: the default per level from its
//! filesystem's free space, an explicit `max_size`, and eviction of the
//! least recently used entries first.
use super::tests::test_cache;
use super::*;
use gaia_testkit::TempDir;
use std::time::{Duration, SystemTime};

const GIB: u64 = 1024 * 1024 * 1024;

#[test]
fn the_default_is_half_the_free_space_plus_what_the_level_holds() {
    assert_eq!(evict::default_max_size(Some(10 * GIB), 0), 5 * GIB);
    assert_eq!(evict::default_max_size(Some(10 * GIB), 30 * GIB), 35 * GIB);
    assert_eq!(
        evict::default_max_size(Some(57 * GIB), 0),
        28 * GIB + GIB / 2
    );
}

#[test]
fn the_default_is_capped_at_40_gib_and_floored_at_2_gib() {
    assert_eq!(evict::default_max_size(Some(200 * GIB), 0), 40 * GIB);
    assert_eq!(evict::default_max_size(Some(200 * GIB), 60 * GIB), 40 * GIB);
    assert_eq!(evict::default_max_size(Some(0), 0), 2 * GIB);
    assert_eq!(evict::default_max_size(Some(GIB), 0), 2 * GIB);
}

#[test]
fn without_a_free_space_reading_the_default_is_the_cap() {
    assert_eq!(evict::default_max_size(None, 10 * GIB), 40 * GIB);
}

#[test]
fn an_explicit_max_size_is_every_levels_budget_and_a_default_is_per_level() {
    let scratch = TempDir::new("budget-levels");
    let mut cache = test_cache(&scratch.join("root"), &[]);
    let project = cache.project.clone();
    let system = cache.system.clone().expect("system level");
    cache.max_size = Some(7 * GIB);
    assert_eq!(cache.budget(&project), 7 * GIB);
    assert_eq!(cache.budget(&system), 7 * GIB);

    cache.max_size = None;
    cache.default_budgets = vec![(project.clone(), 3 * GIB)];
    assert_eq!(cache.budget(&project), 3 * GIB);
    assert_eq!(cache.budget(&system), evict::DEFAULT_MAX_SIZE_CAP);
}

#[test]
fn only_an_explicit_max_size_can_warn_about_space() {
    let scratch = TempDir::new("budget-warn");
    let mut cache = test_cache(&scratch.join("root"), &[]);
    fs::create_dir_all(&cache.project).unwrap();
    fs::create_dir_all(cache.system.as_ref().unwrap()).unwrap();

    cache.max_size = Some(u64::MAX);
    assert!(
        !cache.space_warning().is_empty(),
        "an explicit size larger than the free space warns"
    );
    cache.max_size = Some(1);
    assert!(cache.space_warning().is_empty());
    cache.max_size = None;
    cache.default_budgets = vec![(cache.project.clone(), u64::MAX)];
    assert!(cache.space_warning().is_empty(), "the default never warns");
}

/// An entry of `level` for `name` with `size` bytes, last used at `used`.
fn fake_entry(level: &Path, name: &str, key: &str, size: u64, used: SystemTime) -> PathBuf {
    let (entry, manifest_path) = entry_in(level, name, key, None);
    fs::create_dir_all(&entry).unwrap();
    let manifest = Manifest {
        output_dir: "/out".to_string(),
        dirs: Vec::new(),
        files: Vec::new(),
        size,
        stamps: Vec::new(),
        rewrite: Vec::new(),
        relocatable: true,
    };
    fs::write(&manifest_path, manifest.to_json(name, None)).unwrap();
    fs::File::options()
        .append(true)
        .open(&manifest_path)
        .unwrap()
        .set_modified(used)
        .unwrap();
    entry
}

#[test]
fn eviction_removes_the_least_recently_used_entries_first_until_under_the_limit() {
    let scratch = TempDir::new("budget-evict");
    let mut cache = test_cache(&scratch.join("root"), &[]);
    let project = cache.project.clone();
    fs::create_dir_all(&project).unwrap();
    // Created out of order: the last use, not the creation, decides.
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let newest = fake_entry(&project, "c", "k-c", 10, base + Duration::from_secs(30));
    let oldest = fake_entry(&project, "a", "k-a", 10, base + Duration::from_secs(10));
    let middle = fake_entry(&project, "b", "k-b", 10, base + Duration::from_secs(20));

    // 30 bytes against a limit of 20: the walk stops once it is at 18.
    cache.max_size = Some(20);
    assert_eq!(cache.evict(), (2, 20));
    assert!(!oldest.exists(), "the least recently used goes first");
    assert!(!middle.exists());
    assert!(newest.exists(), "the newest stays");
    assert_eq!(cache.evict(), (0, 0), "nothing more to evict");
}
