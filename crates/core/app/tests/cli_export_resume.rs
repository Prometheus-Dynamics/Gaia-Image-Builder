pub mod support;

use gaia_app::{AppArgs, CommandOutcome, run_finish, run_with_args};
use std::fs;
use std::path::{Path, PathBuf};
use support::{config_path, seed_default_assets, seed_reuse_state, unique_dir};

/// Roots of one run: the seeded workspace, its output and build dirs.
struct Roots {
    root: String,
    out: String,
    build: String,
}

fn fresh_roots(prefix: &str) -> Roots {
    let roots = Roots {
        root: unique_dir(&format!("{prefix}-root")),
        out: unique_dir(&format!("{prefix}-out")),
        build: unique_dir(&format!("{prefix}-build")),
    };
    fs::create_dir_all(&roots.root).expect("workspace root");
    seed_default_assets(&roots.root);
    roots
}

fn run_words(roots: &Roots, extra: &[&str]) -> Vec<String> {
    let mut words = vec![
        "run".to_string(),
        config_path(),
        "--preset".to_string(),
        "ci".to_string(),
        "--set".to_string(),
        "image.allow_fallback=true".to_string(),
        "--set".to_string(),
        format!("workspace.root_dir={}", roots.root),
        "--set".to_string(),
        format!("workspace.out_dir={}", roots.out),
        "--set".to_string(),
        format!("workspace.build_dir={}", roots.build),
    ];
    words.extend(extra.iter().map(|word| word.to_string()));
    words
}

/// A run whose reusable outputs are seeded, so it completes without building.
fn successful_run(roots: &Roots, extra: &[&str]) -> (AppArgs, CommandOutcome) {
    seed_reuse_state(&roots.root, &roots.build, &roots.out);
    let args = AppArgs::parse_from(run_words(roots, extra));
    let outcome = run_with_args(args.clone());
    (args, outcome)
}

fn file_names(dir: &Path) -> Vec<String> {
    let mut names = fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn sha256_of(path: &Path) -> String {
    let output = std::process::Command::new("sha256sum")
        .arg(path)
        .output()
        .expect("sha256sum");
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .expect("digest")
        .to_string()
}

#[test]
fn successful_run_exports_the_primary_image_under_its_versioned_name() {
    let roots = fresh_roots("gaia-export-ok");
    let export_dir = PathBuf::from(unique_dir("gaia-export-dest"));
    let export_arg = export_dir.display().to_string();
    let (args, outcome) = successful_run(&roots, &["--export", &export_arg]);

    assert_eq!(outcome.exit_code(), 0, "{outcome:?}");
    let finish = run_finish(&args, &outcome);
    assert!(!finish.export_failed, "{:?}", finish.lines);

    let CommandOutcome::Ran { report, .. } = &outcome else {
        panic!("expected ran outcome, got {outcome:?}");
    };
    let primary = PathBuf::from(
        report
            .summary
            .primary_image_output
            .clone()
            .expect("primary image output"),
    );
    let name = primary
        .file_name()
        .expect("primary name")
        .to_string_lossy()
        .into_owned();
    // The archive name already carries the build version (2.0.0), so it is kept.
    assert!(name.contains("2.0.0"), "{name}");
    assert_eq!(file_names(&export_dir), std::slice::from_ref(&name));

    let exported = export_dir.join(&name);
    let digest = sha256_of(&exported);
    assert_eq!(digest, sha256_of(&primary));
    let line = finish
        .lines
        .iter()
        .find(|line| line.starts_with("exported: "))
        .expect("exported line");
    assert_eq!(
        line,
        &format!(
            "exported: {}  sha256 {digest}  {} bytes",
            fs::canonicalize(&exported).expect("canonical").display(),
            fs::metadata(&exported).expect("metadata").len()
        )
    );
    assert!(
        finish
            .lines
            .iter()
            .all(|line| !line.starts_with("resume: ")),
        "a successful run gets no resume hint: {:?}",
        finish.lines
    );
    let _ = fs::remove_dir_all(export_dir);
}

#[test]
fn failed_run_exports_nothing_and_ends_with_the_resume_hint() {
    let roots = Roots {
        root: unique_dir("gaia-export-fail-root"),
        out: unique_dir("gaia-export-fail-out"),
        build: unique_dir("gaia-export-fail-build"),
    };
    fs::create_dir_all(&roots.root).expect("workspace root");
    seed_default_assets(&roots.root);
    let export_dir = PathBuf::from(unique_dir("gaia-export-fail-dest"));
    let export_arg = export_dir.display().to_string();
    // The same invocation as the failing run in cli_run_failures, plus export.
    let args = AppArgs::parse_from(vec![
        "run".to_string(),
        config_path(),
        "--preset".to_string(),
        "ci".to_string(),
        "--set".to_string(),
        format!("workspace.root_dir={}", roots.root),
        "--set".to_string(),
        format!("workspace.out_dir={}", roots.out),
        "--set".to_string(),
        format!("workspace.build_dir={}", roots.build),
        "--export".to_string(),
        export_arg,
    ]);
    let outcome = run_with_args(args.clone());
    assert_eq!(outcome.exit_code(), 4, "{outcome:?}");

    let finish = run_finish(&args, &outcome);
    assert!(!finish.export_failed);
    assert!(!export_dir.exists(), "a failed run creates no export dir");
    assert_eq!(
        finish.lines.first().map(String::as_str),
        Some("export: skipped, the run did not succeed; nothing was exported")
    );
    let hint = finish.lines.last().expect("resume hint");
    assert!(
        hint.starts_with(&format!(
            "resume: gaia run {} --preset ci --set ",
            config_path()
        )),
        "{hint}"
    );
    assert!(
        hint.contains(&format!("--set workspace.root_dir={}", roots.root)),
        "{hint}"
    );
    assert!(
        hint.ends_with("# finished work is reused")
            || hint.ends_with("# operations unwound by rollback run again"),
        "{hint}"
    );
    assert_eq!(finish.lines.len(), 2, "{:?}", finish.lines);
}

#[test]
fn a_run_that_ends_with_errors_is_not_exported_even_when_it_built_an_image() {
    let roots = fresh_roots("gaia-export-errors");
    let export_dir = PathBuf::from(unique_dir("gaia-export-errors-dest"));
    let export_arg = export_dir.display().to_string();
    let (args, mut outcome) = successful_run(&roots, &["--export", &export_arg]);
    assert_eq!(outcome.exit_code(), 0);
    if let CommandOutcome::Ran { report, .. } = &mut outcome {
        report.summary.error_count = 1;
    }
    assert_eq!(outcome.exit_code(), 4);

    let finish = run_finish(&args, &outcome);
    assert!(!export_dir.exists());
    assert!(
        finish
            .lines
            .iter()
            .any(|line| line.starts_with("export: skipped")),
        "{:?}",
        finish.lines
    );
    assert!(
        finish
            .lines
            .last()
            .is_some_and(|line| line.starts_with("resume: gaia run ")),
        "{:?}",
        finish.lines
    );
}

#[test]
fn export_collisions_keep_both_images_and_the_resume_hint_is_absent_on_success() {
    let roots = fresh_roots("gaia-export-collide");
    let export_dir = PathBuf::from(unique_dir("gaia-export-collide-dest"));
    let export_arg = export_dir.display().to_string();
    let (args, outcome) = successful_run(&roots, &["--export", &export_arg]);
    assert_eq!(outcome.exit_code(), 0, "{outcome:?}");

    // A second export of the same run finds identical content and reuses it.
    let again = run_finish(&args, &outcome);
    assert!(!again.export_failed);
    assert_eq!(file_names(&export_dir).len(), 1);

    // A different file under the same name gets a numbered name.
    let name = file_names(&export_dir).remove(0);
    fs::write(export_dir.join(&name), b"someone else's image").expect("overwrite target");
    let third = run_finish(&args, &outcome);
    assert!(!third.export_failed, "{:?}", third.lines);
    let names = file_names(&export_dir);
    assert_eq!(names.len(), 2, "{names:?}");
    assert_eq!(
        fs::read(export_dir.join(&name)).expect("kept"),
        b"someone else's image",
        "the existing different file is never overwritten"
    );
    let _ = fs::remove_dir_all(export_dir);
}

/// The `--set` words that point a run's export at `dir` at one config level.
fn export_set(key: &str, dir: &Path) -> Vec<String> {
    vec!["--set".to_string(), format!("{key}={}", dir.display())]
}

fn as_refs(words: &[String]) -> Vec<&str> {
    words.iter().map(String::as_str).collect()
}

#[test]
fn workspace_export_dir_exports_without_the_export_flag() {
    let roots = fresh_roots("gaia-export-cfg-ws");
    let dest = PathBuf::from(unique_dir("gaia-export-cfg-ws-dest"));
    let set = export_set("workspace.export_dir", &dest);
    let (args, outcome) = successful_run(&roots, &as_refs(&set));

    assert_eq!(outcome.exit_code(), 0, "{outcome:?}");
    let finish = run_finish(&args, &outcome);
    assert!(!finish.export_failed, "{:?}", finish.lines);
    assert_eq!(file_names(&dest).len(), 1, "{:?}", file_names(&dest));
    assert!(
        finish
            .lines
            .iter()
            .any(|line| line.starts_with("exported: ")),
        "{:?}",
        finish.lines
    );
    let _ = fs::remove_dir_all(dest);
}

#[test]
fn image_export_dir_beats_the_workspace_default() {
    let roots = fresh_roots("gaia-export-cfg-img");
    let workspace_dest = PathBuf::from(unique_dir("gaia-export-cfg-ws-loses"));
    let image_dest = PathBuf::from(unique_dir("gaia-export-cfg-img-wins"));
    let mut set = export_set("workspace.export_dir", &workspace_dest);
    set.extend(export_set("image.output.export_dir", &image_dest));
    let (args, outcome) = successful_run(&roots, &as_refs(&set));

    assert_eq!(outcome.exit_code(), 0, "{outcome:?}");
    let finish = run_finish(&args, &outcome);
    assert!(!finish.export_failed, "{:?}", finish.lines);
    assert_eq!(file_names(&image_dest).len(), 1);
    assert!(
        !workspace_dest.exists(),
        "the workspace default is not used when the build sets its own"
    );
    let _ = fs::remove_dir_all(image_dest);
}

#[test]
fn export_flag_beats_the_configured_directory() {
    let roots = fresh_roots("gaia-export-cfg-flag");
    let flag_dest = PathBuf::from(unique_dir("gaia-export-cfg-flag-wins"));
    let config_dest = PathBuf::from(unique_dir("gaia-export-cfg-flag-loses"));
    let flag = flag_dest.display().to_string();
    let set = export_set("image.output.export_dir", &config_dest);
    let mut words = as_refs(&set);
    words.extend(["--export", flag.as_str()]);
    let (args, outcome) = successful_run(&roots, &words);

    assert_eq!(outcome.exit_code(), 0, "{outcome:?}");
    let finish = run_finish(&args, &outcome);
    assert!(!finish.export_failed, "{:?}", finish.lines);
    assert_eq!(file_names(&flag_dest).len(), 1);
    assert!(
        !config_dest.exists(),
        "the configured directory is not used"
    );
    let _ = fs::remove_dir_all(flag_dest);
}

#[test]
fn no_export_suppresses_a_configured_export_for_one_run() {
    let roots = fresh_roots("gaia-export-cfg-none");
    let dest = PathBuf::from(unique_dir("gaia-export-cfg-none-dest"));
    let set = export_set("workspace.export_dir", &dest);
    let mut words = as_refs(&set);
    words.push("--no-export");
    let (args, outcome) = successful_run(&roots, &words);

    assert_eq!(outcome.exit_code(), 0, "{outcome:?}");
    assert!(args.usage_errors.is_empty(), "{:?}", args.usage_errors);
    assert!(args.no_export);
    let finish = run_finish(&args, &outcome);
    assert!(!finish.export_failed);
    assert!(
        finish
            .lines
            .iter()
            .all(|line| !line.starts_with("exported: ")),
        "{:?}",
        finish.lines
    );
    assert!(!dest.exists(), "a suppressed export creates no directory");
}

#[test]
fn relative_configured_export_dir_is_taken_from_the_workspace_root() {
    let roots = fresh_roots("gaia-export-cfg-rel");
    let set = vec![
        "--set".to_string(),
        "image.output.export_dir=exports/images".to_string(),
    ];
    let (args, outcome) = successful_run(&roots, &as_refs(&set));

    assert_eq!(outcome.exit_code(), 0, "{outcome:?}");
    let finish = run_finish(&args, &outcome);
    assert!(!finish.export_failed, "{:?}", finish.lines);
    let dest = PathBuf::from(&roots.root).join("exports/images");
    assert_eq!(file_names(&dest).len(), 1, "{:?}", file_names(&dest));
    let _ = fs::remove_dir_all(dest);
}

#[test]
fn a_failed_run_with_a_configured_export_exports_nothing() {
    let roots = Roots {
        root: unique_dir("gaia-export-cfg-fail-root"),
        out: unique_dir("gaia-export-cfg-fail-out"),
        build: unique_dir("gaia-export-cfg-fail-build"),
    };
    fs::create_dir_all(&roots.root).expect("workspace root");
    seed_default_assets(&roots.root);
    let dest = PathBuf::from(unique_dir("gaia-export-cfg-fail-dest"));
    // The same invocation as the failing run in cli_run_failures, plus a
    // configured export: no `image.allow_fallback`, so the run fails.
    let mut words = vec![
        "run".to_string(),
        config_path(),
        "--preset".to_string(),
        "ci".to_string(),
        "--set".to_string(),
        format!("workspace.root_dir={}", roots.root),
        "--set".to_string(),
        format!("workspace.out_dir={}", roots.out),
        "--set".to_string(),
        format!("workspace.build_dir={}", roots.build),
    ];
    words.extend(export_set("workspace.export_dir", &dest));
    let args = AppArgs::parse_from(words);
    let outcome = run_with_args(args.clone());
    assert_eq!(outcome.exit_code(), 4, "run should fail");

    let finish = run_finish(&args, &outcome);
    assert!(!dest.exists(), "a failed run creates no export dir");
    assert_eq!(
        finish.lines.first().map(String::as_str),
        Some("export: skipped, the run did not succeed; nothing was exported")
    );
}

#[test]
fn export_and_no_export_together_are_a_usage_error() {
    let args = AppArgs::parse_from(vec![
        "run".to_string(),
        config_path(),
        "--export".to_string(),
        "/tmp/x".to_string(),
        "--no-export".to_string(),
    ]);
    assert!(
        args.usage_errors
            .iter()
            .any(|error| error.contains("--export and --no-export")),
        "{:?}",
        args.usage_errors
    );
}
