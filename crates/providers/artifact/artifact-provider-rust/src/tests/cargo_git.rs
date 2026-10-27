//! Cargo git dependencies served from declared git sources, with real git
//! repositories on disk. The dependency's URL is `https://example.invalid/`,
//! so the build only works when cargo's fetch is rewritten to the mirror.

use super::*;
use gaia_artifact_providers::ArtifactProviderErrorKind;
use gaia_source_providers::{GIT_MIRROR_CACHE_DIR, remote_git_mirror_dir_name};
use gaia_spec::{CargoGitSourceSpec, SourceId};
use gaia_testkit::TempDir;
use std::path::PathBuf;

const DEP_REPO: &str = "https://example.invalid/dep.git";

fn tools_available() -> bool {
    ["git", "cargo"].iter().all(|tool| {
        Command::new(tool)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "gaia")
        .env("GIT_AUTHOR_EMAIL", "gaia@example.invalid")
        .env("GIT_COMMITTER_NAME", "gaia")
        .env("GIT_COMMITTER_EMAIL", "gaia@example.invalid")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn write_dep_lib(dep: &Path, answer: u32) {
    fs::write(
        dep.join("src/lib.rs"),
        format!("pub fn answer() -> u32 {{ {answer} }}\n"),
    )
    .expect("lib.rs");
}

/// A dependency repository whose locked commit (returned) is not reachable
/// from any branch. That commit answers 42, while `main` answers one. Only a
/// fetch of that exact commit can supply it, as with a commit that was pushed
/// and then left off every branch.
fn dependency_repo(scratch: &TempDir) -> (PathBuf, String) {
    let dep = scratch.join("dep");
    fs::create_dir_all(dep.join("src")).expect("dep src");
    fs::write(
        dep.join("Cargo.toml"),
        "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("dep manifest");
    git(&dep, &["init", "-q", "-b", "main"]);
    write_dep_lib(&dep, 1);
    git(&dep, &["add", "-A"]);
    git(&dep, &["commit", "-q", "-m", "answer one"]);
    git(&dep, &["checkout", "-q", "-b", "side"]);
    write_dep_lib(&dep, 42);
    git(&dep, &["commit", "-q", "-am", "answer forty-two"]);
    let locked = git(&dep, &["rev-parse", "HEAD"]);
    git(&dep, &["checkout", "-q", "main"]);
    git(&dep, &["branch", "-q", "-D", "side"]);
    // Hosts such as GitHub serve any commit by id; a local repository
    // needs this to match.
    git(&dep, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
    (dep, locked)
}

/// The mirror the git source provider keeps for `DEP_REPO`, cloned the way
/// it clones: it has the branches but not the unreachable commit.
fn source_mirror(workspace: &Path, dep: &Path) -> PathBuf {
    let mirror = workspace
        .join(GIT_MIRROR_CACHE_DIR)
        .join(remote_git_mirror_dir_name(DEP_REPO));
    fs::create_dir_all(mirror.parent().expect("cache dir")).expect("cache dir");
    // `--no-local`: a local clone hard-links every object, including the
    // unreachable locked commit; over the protocol only reachable ones come.
    let output = Command::new("git")
        .args(["clone", "-q", "--mirror", "--no-local"])
        .arg(dep)
        .arg(&mirror)
        .output()
        .expect("git clone runs");
    assert!(output.status.success(), "mirror clone failed");
    mirror
}

fn lock_for(commit: &str) -> String {
    format!(
        "version = 4\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\n \"dep\",\n]\n\n[[package]]\nname = \"dep\"\nversion = \"0.1.0\"\nsource = \"git+{DEP_REPO}?rev={commit}#{commit}\"\n"
    )
}

/// An app whose only dependency is the locked git dependency.
fn app_workspace(workspace: &Path, commit: &str) -> PathBuf {
    let app = workspace.join("app");
    fs::create_dir_all(app.join("src")).expect("app src");
    fs::write(
        app.join("Cargo.toml"),
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ndep = {{ git = \"{DEP_REPO}\", rev = \"{commit}\" }}\n"
        ),
    )
    .expect("app manifest");
    fs::write(
        app.join("src/main.rs"),
        "fn main() { println!(\"{}\", dep::answer()); }\n",
    )
    .expect("main.rs");
    fs::write(app.join("Cargo.lock"), lock_for(commit)).expect("Cargo.lock");
    app
}

fn mapping(source: &str) -> CargoGitSourceSpec {
    CargoGitSourceSpec {
        url: DEP_REPO.into(),
        source: SourceId::new(source),
        repo: DEP_REPO.into(),
    }
}

/// The rust artifact `app` with `sources` as its cargo git sources, and a
/// contract whose workspace root is `workspace`.
fn app_contract(
    workspace: &Path,
    app: &Path,
    sources: Vec<CargoGitSourceSpec>,
) -> (ArtifactSpec, ArtifactExecutionContract) {
    let artifact = ArtifactSpec::new(
        "app",
        ArtifactDefinition::Rust(RustArtifactSpec {
            package: "app".into(),
            target_name: Some("app".into()),
            variant: ArtifactVariantSpec::File,
            features: Vec::new(),
            no_default_features: false,
            all_features: false,
            build_group: None,
            group_packages: Vec::new(),
            cargo_git_sources: sources,
            build_env: Vec::new(),
        }),
        None,
        ArtifactOutputSpec {
            path: workspace.join("out/app").display().to_string(),
        },
    );
    let mut contract = ArtifactExecutionContract::from_spec(
        &artifact,
        Some(app.display().to_string()),
        true,
        ArtifactExecutionContract::default_command_policy(),
        gaia_spec::OutputRetentionPolicySpec::default(),
    );
    contract.workspace_root = Some(workspace.display().to_string());
    (artifact, contract)
}

fn env_value(env: &[(String, String)], name: &str) -> Option<String> {
    env.iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

#[test]
fn preparing_a_locked_git_dependency_fetches_its_commit_and_pins_it() {
    if !tools_available() {
        eprintln!("skipped: git or cargo is not on PATH");
        return;
    }
    let scratch = TempDir::new("gaia-rust-cargo-git-prepare");
    let workspace = scratch.join("ws");
    let (dep, locked) = dependency_repo(&scratch);
    let mirror = source_mirror(&workspace, &dep);
    let app = app_workspace(&workspace, &locked);
    let (_, contract) = app_contract(&workspace, &app, vec![mapping("dep")]);

    let commit_in_mirror = |commit: &str| {
        Command::new("git")
            .arg("-C")
            .arg(&mirror)
            .args(["cat-file", "-e", &format!("{commit}^{{commit}}")])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    assert!(
        !commit_in_mirror(&locked),
        "the locked commit must start out missing from the mirror"
    );

    let env = crate::cargo_git::cargo_git_env(
        &[mapping("dep")],
        &contract,
        &app.display().to_string(),
        None,
    )
    .expect("prepared");
    assert!(commit_in_mirror(&locked), "the locked commit is fetched");
    assert_eq!(
        env_value(&env, "CARGO_NET_GIT_FETCH_WITH_CLI").as_deref(),
        Some("true")
    );
    assert_eq!(env_value(&env, "GIT_CONFIG_COUNT").as_deref(), Some("1"));
    assert_eq!(
        env_value(&env, "GIT_CONFIG_KEY_0"),
        Some(format!("url.file://{}.insteadOf", mirror.display()))
    );
    assert_eq!(
        env_value(&env, "GIT_CONFIG_VALUE_0").as_deref(),
        Some(DEP_REPO)
    );
    assert!(
        git(
            &mirror,
            &[
                "show-ref",
                "--verify",
                &format!("refs/heads/gaia-locked/{locked}")
            ]
        )
        .ends_with(&format!("refs/heads/gaia-locked/{locked}"))
    );

    // Running again changes nothing: the commit and the pin are already there.
    let again = crate::cargo_git::cargo_git_env(
        &[mapping("dep")],
        &contract,
        &app.display().to_string(),
        None,
    )
    .expect("prepared again");
    assert_eq!(again, env);
}

#[test]
fn a_locked_commit_the_source_does_not_have_is_a_clear_error() {
    if !tools_available() {
        eprintln!("skipped: git or cargo is not on PATH");
        return;
    }
    let scratch = TempDir::new("gaia-rust-cargo-git-missing");
    let workspace = scratch.join("ws");
    let (dep, _) = dependency_repo(&scratch);
    source_mirror(&workspace, &dep);
    let missing = "f".repeat(40);
    let app = app_workspace(&workspace, &missing);
    let (_, contract) = app_contract(&workspace, &app, vec![mapping("dep")]);

    let error = crate::cargo_git::cargo_git_env(
        &[mapping("dep")],
        &contract,
        &app.display().to_string(),
        None,
    )
    .expect_err("missing commit");
    assert_eq!(error.kind, ArtifactProviderErrorKind::BackendCommand);
    assert!(error.message.contains(DEP_REPO), "{}", error.message);
    assert!(error.message.contains(&missing), "{}", error.message);
    assert!(error.message.contains("'dep'"), "{}", error.message);
}

#[test]
fn a_build_without_a_locked_repository_prepares_nothing() {
    let scratch = TempDir::new("gaia-rust-cargo-git-none");
    let workspace = scratch.join("ws");
    let app = scratch.join("ws/app");
    fs::create_dir_all(&app).expect("app dir");
    fs::write(app.join("Cargo.lock"), "version = 4\n").expect("lock");
    let (_, contract) = app_contract(&workspace, &app, vec![mapping("dep")]);

    let env = crate::cargo_git::cargo_git_env(
        &[mapping("dep")],
        &contract,
        &app.display().to_string(),
        None,
    )
    .expect("nothing to prepare");
    assert!(env.is_empty());
    assert!(!workspace.join(GIT_MIRROR_CACHE_DIR).exists());
}

#[test]
fn cargo_builds_a_locked_git_dependency_through_the_mirror() {
    if !tools_available() {
        eprintln!("skipped: git or cargo is not on PATH");
        return;
    }
    let scratch = TempDir::new("gaia-rust-cargo-git-build");
    let workspace = scratch.join("ws");
    let (dep, locked) = dependency_repo(&scratch);
    source_mirror(&workspace, &dep);
    let app = app_workspace(&workspace, &locked);
    let (artifact, contract) = app_contract(&workspace, &app, vec![mapping("dep")]);
    let ArtifactDefinition::Rust(rust) = &artifact.definition else {
        panic!("rust artifact");
    };
    let flags = CargoFeatureFlags::of(&artifact);
    assert_eq!(flags.git_sources, rust.cargo_git_sources);

    // The same environment and command `run_cargo_build` uses, with cargo's
    // own home in the scratch dir so the test leaves nothing behind.
    let env = crate::cargo_git::cargo_git_env(
        &flags.git_sources,
        &contract,
        &app.display().to_string(),
        None,
    )
    .expect("prepared");
    let target = scratch.join("target");
    let mut command = cargo_build_command(
        &app.display().to_string(),
        &["app".to_string()],
        &flags,
        &contract,
        &target,
    );
    command.envs(env.iter().map(|(key, value)| (key, value)));
    command.env("CARGO_HOME", scratch.join("cargo-home"));
    let output = command.output().expect("cargo runs");
    assert!(
        output.status.success(),
        "cargo build failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let binary = Command::new(target.join("debug/app"))
        .output()
        .expect("built binary runs");
    // The locked commit answers 42; `main` answers 1.
    assert_eq!(String::from_utf8_lossy(&binary.stdout).trim(), "42");
}
