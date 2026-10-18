//! `gaia run` end to end for a build with `[[variants]]`: a git source whose
//! rev differs per variant (and an artifact built from it), a shared path
//! source with a shared artifact, and a starting-point image per variant.

use gaia_app::{AppArgs, CommandOutcome, VariantOutcome, run_with_args};
use gaia_testkit::TempDir;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

const APP_CARGO: &str = "[package]\nname = \"variant-app\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"variant-app\"\npath = \"src/main.rs\"\n\n[workspace]\n";
const SHARED_CARGO: &str = "[package]\nname = \"shared-tool\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"shared-tool\"\npath = \"src/main.rs\"\n\n[workspace]\n";

struct Scenario {
    root: PathBuf,
    build: PathBuf,
    _scratch: TempDir,
}

impl Scenario {
    fn build_dir(&self) -> PathBuf {
        self.root.join("build")
    }

    fn out_dir(&self) -> PathBuf {
        self.root.join("out")
    }

    fn app_output(&self, variant: &str) -> PathBuf {
        self.out_dir()
            .join("artifacts/variants")
            .join(variant)
            .join("variant-app")
    }

    fn shared_output(&self) -> PathBuf {
        self.out_dir().join("artifacts/shared-tool")
    }

    fn app_source(&self, variant: &str) -> PathBuf {
        self.build_dir()
            .join("sources/variants")
            .join(variant)
            .join("app-source")
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Gaia Test",
            "-c",
            "user.email=gaia@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A scratch root with a git repo of the app at two commits, a shared path
/// source, a seed rootfs and the build file. `v2` overrides the app rev.
fn scenario(name: &str) -> Scenario {
    let scratch = TempDir::new(&format!("gaia-variants-e2e-{name}"));
    let root = scratch.path().to_path_buf();

    let repo = root.join("app-repo");
    fs::create_dir_all(repo.join("src")).expect("repo src");
    git(
        &root,
        &["init", "-q", "-b", "main", repo.to_str().expect("utf-8")],
    );
    fs::write(repo.join("Cargo.toml"), APP_CARGO).expect("app cargo");
    fs::write(
        repo.join("src/main.rs"),
        "fn main() { println!(\"variant-app rev one\"); }\n",
    )
    .expect("app main v1");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "rev one"]);
    let rev_one = git(&repo, &["rev-parse", "HEAD"]);
    fs::write(
        repo.join("src/main.rs"),
        "fn main() { println!(\"variant-app rev two\"); }\n",
    )
    .expect("app main v2");
    git(&repo, &["commit", "-q", "-am", "rev two"]);
    let rev_two = git(&repo, &["rev-parse", "HEAD"]);

    let shared = root.join("shared");
    fs::create_dir_all(shared.join("src")).expect("shared src");
    fs::write(shared.join("Cargo.toml"), SHARED_CARGO).expect("shared cargo");
    fs::write(
        shared.join("src/main.rs"),
        "fn main() { println!(\"shared-tool\"); }\n",
    )
    .expect("shared main");

    let rootfs = root.join("seed-rootfs/etc");
    fs::create_dir_all(&rootfs).expect("seed rootfs");
    fs::write(rootfs.join("motd"), "seed\n").expect("seed motd");

    let root_str = root.display();
    let build = root.join("variant-e2e.toml");
    fs::write(
        &build,
        format!(
            r#"
build_name = "variant-e2e"
version = "1.0.0"
description = "Variant end-to-end fixture"
target = "local"
profile = "release"

[inputs.app_rev]
kind = "string"
default = "{rev_one}"

[workspace]
root_dir = "{root_str}"
build_dir = "{root_str}/build"
out_dir = "{root_str}/out"

[[sources]]
id = "app-source"
kind = "git"
repo = "file://{root_str}/app-repo"
rev = "${{input.app_rev}}"
refresh = "never"
pin = "locked"

[[sources]]
id = "shared-source"
kind = "path"
path = "shared"
identity_ignore = ["target", ".gaia", "Cargo.lock"]
refresh = "never"
pin = "locked"

[[artifacts]]
id = "variant-app"
kind = "rust"
package = "variant-app"
source = "app-source"
profile = "release"
install_name = "variant-app"
install_class = "binary"
output_path = "${{workspace.out_dir}}/artifacts/variant-app"

[[artifacts]]
id = "shared-tool"
kind = "rust"
package = "shared-tool"
source = "shared-source"
profile = "release"
install_name = "shared-tool"
install_class = "binary"
output_path = "${{workspace.out_dir}}/artifacts/shared-tool"

[image]
kind = "starting-point"
rootfs_path = "{root_str}/seed-rootfs"
rootfs_validation_mode = "require-directory"
output_mode = "copy-and-archive"

[image.output]
collect_dir = "${{workspace.out_dir}}/images"
archive_name = "${{build.name}}-${{build.version}}.tar"
emit_report = true

[[variants]]
id = "v2"
overrides = [["input.app_rev", "{rev_two}"], ["image.output.archive_name", "variant-e2e-v2.tar"]]
"#
        ),
    )
    .expect("build file");

    Scenario {
        root,
        build,
        _scratch: scratch,
    }
}

fn run_args(scenario: &Scenario, extra: &[&str]) -> AppArgs {
    let mut args = vec![
        "run".to_string(),
        scenario.build.display().to_string(),
        "--set".to_string(),
        "policy.providers.rust.allow_nested_build=true".to_string(),
    ];
    for item in extra {
        args.push((*item).to_string());
    }
    AppArgs::parse_from(args)
}

fn run_variants(scenario: &Scenario, extra: &[&str]) -> Vec<VariantOutcome> {
    match run_with_args(run_args(scenario, extra)) {
        CommandOutcome::Variants { outcomes, .. } => outcomes,
        other => panic!("expected variant outcomes, got {other:?}"),
    }
}

fn ran_summary(outcome: &VariantOutcome) -> (usize, usize) {
    match &outcome.outcome {
        CommandOutcome::Ran { report, .. } => (
            report.summary.completed_operations,
            report.summary.reused_operations,
        ),
        other => panic!("variant {} did not run cleanly: {other:?}", outcome.id),
    }
}

fn assert_all_ran(outcomes: &[VariantOutcome]) {
    for outcome in outcomes {
        assert_eq!(
            outcome.outcome.exit_code(),
            0,
            "variant {} failed: {:?}",
            outcome.id,
            outcome.outcome
        );
    }
}

/// Whether a reuse state file records this operation as completed. Matches
/// whole lines, so the duration records (`dur=<id>;<ms>`) do not count.
fn has_entry(state: &str, id: &str) -> bool {
    state.lines().any(|line| line == id)
}

fn modified(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or_else(|error| panic!("{} missing: {error}", path.display()))
}

fn binary_output(path: &Path) -> String {
    let output = Command::new(path).output().expect("artifact runs");
    assert!(output.status.success(), "{} failed", path.display());
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn variants_build_their_own_sources_and_reuse_shared_work() {
    let scenario = scenario("full");

    // First run: the base (`default`) builds rev one, `v2` builds rev two.
    let first = run_variants(&scenario, &[]);
    assert_eq!(
        first
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        ["default", "v2"]
    );
    assert_all_ran(&first);
    let (_, default_reused) = ran_summary(&first[0]);
    assert_eq!(default_reused, 0, "the first variant reuses nothing");
    let (_, v2_reused) = ran_summary(&first[1]);
    assert!(
        v2_reused >= 2,
        "v2 reuses the shared source and artifact, reused {v2_reused}"
    );

    // Variant-specific artifacts: distinct paths, each with its own rev.
    let default_app = scenario.app_output("default");
    let v2_app = scenario.app_output("v2");
    assert_ne!(default_app, v2_app);
    assert_eq!(binary_output(&default_app), "variant-app rev one");
    assert_eq!(binary_output(&v2_app), "variant-app rev two");
    assert_eq!(binary_output(&scenario.shared_output()), "shared-tool");

    // Variant-specific checkouts, and the shared one stays at the shared path.
    let default_main = fs::read_to_string(scenario.app_source("default").join("src/main.rs"))
        .expect("default checkout");
    assert!(default_main.contains("rev one"), "{default_main}");
    let v2_main =
        fs::read_to_string(scenario.app_source("v2").join("src/main.rs")).expect("v2 checkout");
    assert!(v2_main.contains("rev two"), "{v2_main}");
    assert!(scenario.build_dir().join("sources/shared-source").exists());
    assert!(!scenario.build_dir().join("sources/app-source").exists());

    // Shared state holds the shared operations; each variant has its own.
    let shared_state = fs::read_to_string(scenario.out_dir().join(".gaia/variant-e2e.reuse-state"))
        .expect("shared reuse state");
    assert!(
        has_entry(&shared_state, "source:shared-source"),
        "{shared_state}"
    );
    assert!(
        has_entry(&shared_state, "artifact:shared-tool"),
        "{shared_state}"
    );
    assert!(
        !has_entry(&shared_state, "source:app-source"),
        "{shared_state}"
    );
    let v2_state = fs::read_to_string(scenario.out_dir().join(".gaia/variant-e2e@v2.reuse-state"))
        .expect("v2 reuse state");
    assert!(has_entry(&v2_state, "artifact:variant-app"), "{v2_state}");
    // The variant's own file also keeps the shared entries it reused, so it
    // is complete on its own.
    assert!(has_entry(&v2_state, "artifact:shared-tool"), "{v2_state}");

    // Per-variant reports exist and do not overwrite each other.
    let report_paths = first
        .iter()
        .flat_map(|item| match &item.outcome {
            CommandOutcome::Ran { report_outputs, .. } => report_outputs
                .files
                .iter()
                .map(|file| file.path.clone())
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect::<Vec<_>>();
    assert!(!report_paths.is_empty(), "reports were written");
    for path in &report_paths {
        assert!(path.is_file(), "{} exists", path.display());
    }
    assert!(
        report_paths
            .iter()
            .any(|path| path.display().to_string().contains("@v2")),
        "{report_paths:?}"
    );
    assert!(
        report_paths
            .iter()
            .any(|path| path.display().to_string().contains("@default")),
        "{report_paths:?}"
    );

    // Second run: nothing that builds an artifact runs again.
    let before = [
        scenario.shared_output(),
        default_app.clone(),
        v2_app.clone(),
    ]
    .map(|path| modified(&path));
    let second = run_variants(&scenario, &[]);
    assert_all_ran(&second);
    let after = [scenario.shared_output(), default_app, v2_app].map(|path| modified(&path));
    assert_eq!(before, after, "second run rebuilt an artifact");
    for item in &second {
        let (_, reused) = ran_summary(item);
        assert!(reused >= 1, "variant {} reused nothing", item.id);
    }
}

#[test]
fn selecting_one_variant_runs_its_own_operations_and_reuses_shared_ones() {
    let scenario = scenario("select");

    // Only v2: the shared source and artifact are built here, the base's own
    // checkout and artifact are not.
    let only_v2 = run_variants(&scenario, &["--variant", "v2"]);
    assert_eq!(only_v2.len(), 1);
    assert_eq!(only_v2[0].id, "v2");
    assert_all_ran(&only_v2);
    assert!(scenario.app_output("v2").is_file());
    assert!(scenario.shared_output().is_file());
    assert!(!scenario.app_output("default").exists());
    assert!(!scenario.app_source("default").exists());
    let shared_built = modified(&scenario.shared_output());

    // Only the base next: its own rev-one artifact is built, and the shared
    // artifact is reused, not rebuilt.
    let only_default = run_variants(&scenario, &["--variant", "default"]);
    assert_eq!(only_default.len(), 1);
    assert_all_ran(&only_default);
    let (_, reused) = ran_summary(&only_default[0]);
    assert!(
        reused >= 1,
        "default reused the shared work, reused {reused}"
    );
    assert_eq!(
        binary_output(&scenario.app_output("default")),
        "variant-app rev one"
    );
    assert_eq!(modified(&scenario.shared_output()), shared_built);
    assert_eq!(
        binary_output(&scenario.app_output("v2")),
        "variant-app rev two"
    );

    // An unknown name is refused before anything runs.
    match run_with_args(run_args(&scenario, &["--variant", "nope"])) {
        CommandOutcome::Failed { message } => {
            assert!(message.contains("unknown variant 'nope'"), "{message}")
        }
        other => panic!("expected a failure, got {other:?}"),
    }
}

/// Reuse state recorded before fingerprint schemes (no `scheme=` line) is
/// read without an error and re-evaluated once: the next run executes the
/// work again, and the run after it reuses it with the config unchanged.
#[test]
fn reuse_state_of_an_older_fingerprint_scheme_rebuilds_once_then_reuses() {
    let scenario = scenario("scheme");
    let first = run_variants(&scenario, &["--variant", "v2"]);
    assert_all_ran(&first);
    let state_dir = scenario.out_dir().join(".gaia");
    let mut rewritten = 0;
    for entry in fs::read_dir(&state_dir).expect("state dir") {
        let path = entry.expect("state entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("reuse-state") {
            continue;
        }
        let state = fs::read_to_string(&path).expect("reuse state");
        assert!(
            state.lines().any(|line| line.starts_with("scheme=")),
            "{state}"
        );
        let legacy = state
            .lines()
            .filter(|line| !line.starts_with("scheme="))
            .map(|line| format!("{line}\n"))
            .collect::<String>();
        fs::write(&path, legacy).expect("legacy reuse state");
        rewritten += 1;
    }
    assert!(
        rewritten >= 1,
        "no reuse state under {}",
        state_dir.display()
    );

    let upgraded = run_variants(&scenario, &["--variant", "v2"]);
    assert_all_ran(&upgraded);
    let (_, reused) = ran_summary(&upgraded[0]);
    assert_eq!(reused, 0, "an older scheme's state is never reused");

    let next = run_variants(&scenario, &["--variant", "v2"]);
    assert_all_ran(&next);
    let (_, reused) = ran_summary(&next[0]);
    assert!(
        reused >= 2,
        "the unchanged config is reused, reused {reused}"
    );
}

/// `gaia validate` checks every variant as `run` resolves it: a variant
/// publishing to the default variant's archive is reported, with the
/// variant named, and `--variant` must name a declared variant.
#[test]
fn validate_checks_every_variant() {
    let scenario = scenario("validate");
    let validate = |extra: &[&str]| {
        let mut args = vec!["validate".to_string(), scenario.build.display().to_string()];
        args.extend(extra.iter().map(|item| (*item).to_string()));
        run_with_args(AppArgs::parse_from(args))
    };

    match validate(&[]) {
        CommandOutcome::Validated { validation, .. } => {
            assert!(validation.errors.is_empty(), "{:?}", validation.errors);
        }
        other => panic!("expected a validation, got {other:?}"),
    }

    // v2 publishing the default variant's archive name collides.
    let colliding = fs::read_to_string(&scenario.build)
        .expect("build file")
        .replace("\"variant-e2e-v2.tar\"", "\"variant-e2e-1.0.0.tar\"");
    fs::write(&scenario.build, colliding).expect("colliding build file");
    match validate(&[]) {
        CommandOutcome::Validated { validation, .. } => {
            let errors = validation.errors.join("\n");
            assert!(
                errors
                    .contains("variants 'default' and 'v2' would publish to the same image output"),
                "{errors}"
            );
        }
        other => panic!("expected a validation, got {other:?}"),
    }

    match validate(&["--variant", "nope"]) {
        CommandOutcome::Failed { message } => {
            assert!(message.contains("unknown variant 'nope'"), "{message}");
        }
        other => panic!("expected a failure, got {other:?}"),
    }
}
