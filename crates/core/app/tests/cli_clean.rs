pub mod support;

use gaia_app::{AppArgs, CommandOutcome, run_with_args};
use std::fs;
use std::path::PathBuf;

use support::{unique_dir, write_temp_build};

#[test]
fn clean_removes_build_and_out_without_running_build() {
    let root_dir = unique_dir("gaia-cli-clean-root");
    let build_dir = unique_dir("gaia-cli-clean-build");
    let out_dir = unique_dir("gaia-cli-clean-out");
    fs::create_dir_all(&root_dir).expect("workspace root");
    fs::create_dir_all(PathBuf::from(&build_dir).join("nested")).expect("build dir");
    fs::create_dir_all(&out_dir).expect("out dir");
    fs::write(PathBuf::from(&build_dir).join("nested/file.txt"), "build").expect("build file");
    fs::write(PathBuf::from(&out_dir).join("image.tar"), "out").expect("out file");

    let build = write_temp_build(&format!(
        r#"
build_name = "clean-default"

[workspace]
root_dir = "{root_dir}"
build_dir = "{build_dir}"
out_dir = "{out_dir}"
"#
    ));

    let outcome = run_with_args(AppArgs::parse_from(["clean", &build]));

    match outcome {
        CommandOutcome::Cleaned { report, .. } => {
            assert!(!report.dry_run);
            assert_eq!(report.removed.len(), 2);
        }
        other => panic!("expected cleaned outcome, got {other:?}"),
    }
    assert!(!PathBuf::from(&build_dir).exists());
    assert!(!PathBuf::from(&out_dir).exists());
}

#[test]
fn clean_caches_prunes_orphaned_mirrors_and_leftovers_with_sizes() {
    let root_dir = unique_dir("gaia-cli-clean-caches-root");
    let root = PathBuf::from(&root_dir);
    let build_dir = root.join("build");
    let repo = "https://example.invalid/orion.git";
    let git_cache = root.join(".gaia/cache/git");
    let used_mirror = git_cache.join(gaia_source_providers::remote_git_mirror_dir_name(repo));
    let orphan_mirror = git_cache.join("repo-0000000000000000.git");
    let old_layout_mirror = git_cache.join("orion-main-1234.git");
    let preserved = build_dir.join("sources/.orion.gaia-preserved");
    let refresh =
        build_dir.join("image/buildroot-output/build/buildroot-fs/squashfs/target.refresh");
    let downloads = root.join(".gaia/cache/downloads/sha256");
    for dir in [
        &used_mirror,
        &orphan_mirror,
        &old_layout_mirror,
        &preserved,
        &refresh,
        &downloads,
    ] {
        fs::create_dir_all(dir).expect("cache dir");
    }
    fs::write(orphan_mirror.join("pack"), vec![0u8; 1000]).expect("orphan data");
    fs::write(preserved.join("state"), vec![0u8; 24]).expect("preserved data");
    fs::write(downloads.join("abc"), "cached").expect("download");

    let build = write_temp_build(&format!(
        r#"
build_name = "clean-caches"

[workspace]
root_dir = "{root_dir}"
build_dir = "{build}"
out_dir = "{root_dir}/out"

[[sources]]
id = "orion"
kind = "git"
repo = "{repo}"
branch = "main"
"#,
        build = build_dir.display()
    ));

    let dry_run = run_with_args(AppArgs::parse_from([
        "clean",
        &build,
        "--target",
        "caches",
        "--dry-run",
    ]));
    match dry_run {
        CommandOutcome::Cleaned { report, .. } => {
            assert!(report.dry_run);
            assert_eq!(report.removed.len(), 4, "{:?}", report.removed);
            assert!(report.freed_bytes() >= 1024);
        }
        other => panic!("expected cleaned outcome, got {other:?}"),
    }
    assert!(orphan_mirror.exists());

    let cleaned = run_with_args(AppArgs::parse_from(["clean", &build, "--target", "caches"]));
    match cleaned {
        CommandOutcome::Cleaned { report, .. } => {
            assert_eq!(report.freed_bytes(), 1024);
        }
        other => panic!("expected cleaned outcome, got {other:?}"),
    }
    assert!(used_mirror.exists(), "mirror of a current source is kept");
    assert!(!orphan_mirror.exists());
    assert!(!old_layout_mirror.exists());
    assert!(!preserved.exists());
    assert!(!refresh.exists());
    assert!(
        downloads.join("abc").exists(),
        "shared caches need --all-caches"
    );
    assert!(
        build_dir.join("sources").exists(),
        "build dir is not a cache"
    );

    let all = run_with_args(AppArgs::parse_from(["clean", &build, "--all-caches"]));
    assert!(matches!(all, CommandOutcome::Cleaned { .. }), "{all:?}");
    assert!(!used_mirror.exists());
    assert!(!root.join(".gaia/cache/downloads").exists());
    assert!(
        build_dir.exists(),
        "--all-caches alone must not clean the build dir"
    );
}

#[test]
fn clean_uses_configured_profile_and_supports_dry_run() {
    let root_dir = unique_dir("gaia-cli-clean-profile-root");
    let build_dir = unique_dir("gaia-cli-clean-profile-build");
    let out_dir = unique_dir("gaia-cli-clean-profile-out");
    let cache_dir = PathBuf::from(&root_dir).join(".cache/gaia");
    fs::create_dir_all(&build_dir).expect("build dir");
    fs::create_dir_all(&out_dir).expect("out dir");
    fs::create_dir_all(&cache_dir).expect("cache dir");
    fs::write(cache_dir.join("state.txt"), "cache").expect("cache file");

    let build = write_temp_build(&format!(
        r#"
build_name = "clean-profile"

[workspace]
root_dir = "{root_dir}"
build_dir = "{build_dir}"
out_dir = "{out_dir}"

[clean]
default = "cache"

[clean.profiles.cache]
paths = [".cache/gaia"]
"#
    ));

    let dry_run = run_with_args(AppArgs::parse_from(["clean", &build, "--dry-run"]));
    match dry_run {
        CommandOutcome::Cleaned { report, .. } => {
            assert!(report.dry_run);
            assert_eq!(report.removed, vec![cache_dir.clone()]);
        }
        other => panic!("expected cleaned outcome, got {other:?}"),
    }
    assert!(cache_dir.exists());

    let cleaned = run_with_args(AppArgs::parse_from(["clean", &build]));
    match cleaned {
        CommandOutcome::Cleaned { report, .. } => {
            assert!(!report.dry_run);
            assert_eq!(report.removed, vec![cache_dir.clone()]);
        }
        other => panic!("expected cleaned outcome, got {other:?}"),
    }
    assert!(!cache_dir.exists());
    assert!(PathBuf::from(&build_dir).exists());
    assert!(PathBuf::from(&out_dir).exists());
}
