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
