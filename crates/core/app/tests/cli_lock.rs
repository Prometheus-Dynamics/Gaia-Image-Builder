pub mod support;

use gaia_app::{AppArgs, CommandOutcome, run_with_args};
use gaia_plan::{OperationKind, operation_fingerprint};
use gaia_spec::{SourceDefinition, SourceId};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use support::unique_dir;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn commit(repo: &Path, contents: &str) -> String {
    fs::write(repo.join("VERSION"), contents).expect("write");
    git(repo, &["add", "."]);
    git(repo, &["commit", "-m", contents]);
    git(repo, &["rev-parse", "HEAD"])
}

fn locked_commit(build: &str) -> Option<String> {
    let spec = gaia_config::try_resolve_config(build).expect("resolve");
    match &spec.sources[0].definition {
        SourceDefinition::Git(git) => git.locked_commit.clone(),
        _ => panic!("expected git source"),
    }
}

fn source_fingerprint(build: &str) -> u64 {
    let spec = gaia_config::try_resolve_config(build).expect("resolve");
    operation_fingerprint(
        &spec,
        &OperationKind::MaterializeSource {
            source_id: SourceId::new("orion"),
        },
    )
}

#[test]
fn lock_pins_git_sources_and_update_moves_them() {
    let root = PathBuf::from(unique_dir("gaia-cli-lock"));
    let repo = root.join("orion");
    fs::create_dir_all(&repo).expect("repo dir");
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.email", "gaia@example.com"]);
    git(&repo, &["config", "user.name", "Gaia Test"]);
    let first = commit(&repo, "one");

    let configs = root.join("configs/builds");
    fs::create_dir_all(&configs).expect("configs dir");
    let build_path = configs.join("cm5.toml");
    fs::write(
        &build_path,
        format!(
            r#"
build_name = "lock-e2e"

[workspace]
root_dir = "{root}"
build_dir = "{root}/build"
out_dir = "{root}/out"

[[sources]]
id = "orion"
kind = "git"
repo = "file://{repo}"
branch = "main"
pin = "locked"
"#,
            root = root.display(),
            repo = repo.display()
        ),
    )
    .expect("build config");
    let build = build_path.display().to_string();

    assert_eq!(
        locked_commit(&build),
        None,
        "no lockfile keeps today's behavior"
    );
    let unlocked_fingerprint = source_fingerprint(&build);

    let outcome = run_with_args(AppArgs::parse_from(["lock", &build]));
    match &outcome {
        CommandOutcome::Locked { report, .. } => {
            assert_eq!(report.lockfile, configs.join("cm5.gaia.lock"));
            assert_eq!(report.entries.len(), 1);
            assert_eq!(report.entries[0].commit, first);
        }
        other => panic!("expected locked outcome, got {other:?}"),
    }
    let lockfile = fs::read_to_string(configs.join("cm5.gaia.lock")).expect("lockfile");
    assert!(lockfile.contains("ref = \"branch:main\""));
    assert_eq!(locked_commit(&build).as_deref(), Some(first.as_str()));
    let first_fingerprint = source_fingerprint(&build);
    assert_ne!(first_fingerprint, unlocked_fingerprint);

    // New upstream commits do not move the lock or the fingerprint.
    let second = commit(&repo, "two");
    let outcome = run_with_args(AppArgs::parse_from(["lock", &build]));
    assert!(
        matches!(outcome, CommandOutcome::Locked { .. }),
        "{outcome:?}"
    );
    assert_eq!(locked_commit(&build).as_deref(), Some(first.as_str()));
    assert_eq!(source_fingerprint(&build), first_fingerprint);

    let outcome = run_with_args(AppArgs::parse_from(["lock", &build, "--update", "orion"]));
    assert!(
        matches!(outcome, CommandOutcome::Locked { .. }),
        "{outcome:?}"
    );
    assert_eq!(locked_commit(&build).as_deref(), Some(second.as_str()));
    assert_ne!(source_fingerprint(&build), first_fingerprint);

    let outcome = run_with_args(AppArgs::parse_from(["lock", &build, "--update", "nope"]));
    assert!(
        matches!(outcome, CommandOutcome::Failed { .. }),
        "{outcome:?}"
    );

    let misuse = run_with_args(AppArgs::parse_from(["run", &build, "--update"]));
    assert!(
        matches!(misuse, CommandOutcome::Failed { .. }),
        "{misuse:?}"
    );
}

#[test]
fn lock_pins_unpinned_import_sources_so_resolution_works() {
    let root = PathBuf::from(unique_dir("gaia-cli-lock-import"));
    let repo = root.join("atlas");
    fs::create_dir_all(&repo).expect("repo dir");
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.email", "gaia@example.com"]);
    git(&repo, &["config", "user.name", "Gaia Test"]);
    fs::write(
        repo.join("layer.toml"),
        "[[stage.env_sets]]\nid = \"raze-env\"\nname = \"raze\"\nentries = [[\"RAZE\", \"1\"]]\n",
    )
    .expect("layer");
    let first = commit(&repo, "one");

    let configs = root.join("configs/builds");
    fs::create_dir_all(&configs).expect("configs dir");
    let build_path = configs.join("cm5.toml");
    fs::write(
        &build_path,
        format!(
            r#"
build_name = "lock-import"
imports = [{{ source = "atlas", path = "layer.toml" }}]

[workspace]
root_dir = "{root}"
build_dir = "{root}/build"
out_dir = "{root}/out"

[[sources]]
id = "atlas"
kind = "git"
repo = "file://{repo}"
"#,
            root = root.display(),
            repo = repo.display()
        ),
    )
    .expect("build config");
    let build = build_path.display().to_string();

    let error = gaia_config::try_resolve_config(&build).expect_err("unpinned import");
    assert!(error.to_string().contains("gaia lock"), "{error}");

    let outcome = run_with_args(AppArgs::parse_from(["lock", &build]));
    assert!(
        matches!(outcome, CommandOutcome::Locked { .. }),
        "{outcome:?}"
    );
    // A new upstream commit does not move the locked import.
    commit(&repo, "two");
    let spec = gaia_config::try_resolve_config(&build).expect("resolve after lock");
    assert_eq!(
        spec.selection.import_sources[0].identity,
        format!("git:file://{}@{first}", repo.display())
    );
    assert!(
        spec.stage
            .env_sets
            .iter()
            .any(|env_set| env_set.id.as_str() == "raze-env")
    );
}
