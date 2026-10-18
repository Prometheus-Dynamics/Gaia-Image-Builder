//! Git checkouts that are updated in place: they match a fresh clone, keep
//! Gaia's own state, clone again on any error, fetch a mirror only when it
//! lacks the wanted commit, and identify a clean checkout by its tree id.

use super::*;
use crate::git::resolve_remote_checkout;
use gaia_spec::{CleanPolicy, GitBackendSpec, SourceSpec, WorkspaceSpec};
use gaia_testkit::TempDir;
use std::cell::Cell;

/// A build whose workspace is the scratch dir holding `build_dir`, so the
/// git mirror cache lands in scratch too.
pub(crate) fn spec_for(build_dir: &Path) -> ResolvedBuildSpec {
    let mut spec = ResolvedBuildSpec::new("test");
    spec.workspace = WorkspaceSpec {
        root_dir: build_dir
            .parent()
            .expect("scratch dir")
            .display()
            .to_string(),
        build_dir: build_dir.display().to_string(),
        out_dir: build_dir.join("out").display().to_string(),
        clean_policy: CleanPolicy::None,
        named_paths: Vec::new(),
    };
    spec
}

pub(crate) fn git_spec(repo: String, locked: Option<String>) -> GitSourceSpec {
    GitSourceSpec {
        repo,
        branch: Some("main".into()),
        tag: None,
        rev: None,
        subdir: None,
        update: true,
        refresh_policy: SourceRefreshPolicySpec::Always,
        pin_policy: SourcePinPolicySpec::Floating,
        locked_commit: locked,
        git_backend: GitBackendSpec::Host,
    }
}

fn source_for(repo: &Path, locked: Option<String>) -> SourceSpec {
    SourceSpec::new(
        "in-place-upstream",
        SourceDefinition::Git(git_spec(format!("file://{}", repo.display()), locked)),
    )
}

pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git command");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

pub(crate) fn init_repo(dir: &Path) {
    fs::create_dir_all(dir).expect("repo dir");
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "gaia@example.com"]);
    git(dir, &["config", "user.name", "Gaia Test"]);
}

/// Stages everything (deletions included) and commits; returns the new HEAD.
pub(crate) fn commit_all(dir: &Path, message: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}

fn run_source(spec: &ResolvedBuildSpec, source: &SourceSpec) {
    GitSourceProvider
        .execute_source(spec, source, None, None)
        .expect("git source execution");
}

/// Every tracked file's path and content.
fn tracked_files(dir: &Path) -> Vec<(String, String)> {
    git(dir, &["ls-files"])
        .lines()
        .map(|path| {
            let content = fs::read_to_string(dir.join(path)).expect("tracked file");
            (path.to_string(), content)
        })
        .collect()
}

fn fresh_clone(repo: &Path, scratch: &TempDir) -> PathBuf {
    let fresh = scratch.join("fresh-clone");
    let output = std::process::Command::new("git")
        .arg("clone")
        .arg("-q")
        .arg(repo)
        .arg(&fresh)
        .output()
        .expect("git clone");
    assert!(output.status.success(), "fresh clone failed");
    fresh
}

fn counting(count: &Cell<u32>) -> impl FnOnce() -> Result<(), SourceProviderError> + '_ {
    move || {
        count.set(count.get() + 1);
        Ok(())
    }
}

#[test]
fn in_place_update_matches_a_fresh_clone_and_keeps_gaia_state() {
    let scratch = TempDir::new("gaia-in-place");
    let repo = scratch.join("upstream");
    init_repo(&repo);
    fs::write(repo.join("a.txt"), "one").expect("file");
    fs::create_dir_all(repo.join("dir")).expect("dir");
    fs::write(repo.join("dir/b.txt"), "one").expect("file");
    commit_all(&repo, "one");

    let spec = spec_for(&scratch.join("build"));
    let source = source_for(&repo, None);
    run_source(&spec, &source);
    let checkout = materialized_dir(&spec, &source);

    // Left over from before: build state, an untracked file, a local edit,
    // and a file inside .git that only a clone from scratch would remove.
    fs::create_dir_all(checkout.join(".gaia/cargo-target")).expect("gaia dir");
    fs::write(checkout.join(".gaia/cargo-target/keep.txt"), "state").expect("gaia state");
    fs::write(checkout.join("stray.txt"), "untracked").expect("stray");
    fs::write(checkout.join("a.txt"), "edited").expect("edit");
    fs::write(checkout.join(".git/gaia-test-marker"), "kept").expect("marker");

    fs::write(repo.join("a.txt"), "two").expect("file");
    fs::remove_dir_all(repo.join("dir")).expect("remove dir");
    fs::create_dir_all(repo.join("c")).expect("dir");
    fs::write(repo.join("c/d.txt"), "new").expect("file");
    let commit = commit_all(&repo, "two");

    run_source(&spec, &source);

    assert_eq!(git_head_commit(&checkout).as_deref(), Some(commit.as_str()));
    assert!(
        checkout.join(".git/gaia-test-marker").exists(),
        "the checkout was cloned again instead of updated in place"
    );
    assert_eq!(
        tracked_files(&checkout),
        tracked_files(&fresh_clone(&repo, &scratch))
    );
    assert!(!checkout.join("stray.txt").exists());
    assert!(!checkout.join("dir").exists());
    assert_eq!(
        fs::read_to_string(checkout.join(".gaia/cargo-target/keep.txt")).expect("gaia state"),
        "state"
    );
    // Only Gaia's own files are untracked.
    for line in git(&checkout, &["status", "--porcelain"]).lines() {
        let path = line.strip_prefix("?? ").expect("untracked entry");
        assert!(
            matches!(
                path,
                ".gaia/"
                    | "source.txt"
                    | ".gaia-source-state.txt"
                    | "refs.txt"
                    | "selected-subdir.txt"
                    | "remote-resolution-error.txt"
            ),
            "unexpected untracked '{path}'"
        );
    }
    let tree = git(&checkout, &["rev-parse", "HEAD^{tree}"]);
    let state = fs::read_to_string(checkout.join(".gaia-source-state.txt")).expect("state");
    assert!(
        state.contains(&format!("materialized_tree_digest=git-tree:{tree}")),
        "{state}"
    );
}

#[test]
fn checkout_content_digest_uses_the_tree_only_while_the_checkout_is_clean() {
    let scratch = TempDir::new("gaia-digest-clean");
    let repo = scratch.join("upstream");
    init_repo(&repo);
    fs::write(repo.join("a.txt"), "one").expect("file");
    commit_all(&repo, "one");
    let spec = spec_for(&scratch.join("build"));
    let source = source_for(&repo, None);
    run_source(&spec, &source);
    let checkout = materialized_dir(&spec, &source);
    let tree = git(&checkout, &["rev-parse", "HEAD^{tree}"]);
    let hashed = |dir: &Path| {
        tree_digest(
            dir,
            &[".git", ".gaia", "source.txt", ".gaia-source-state.txt"],
        )
    };

    // Gaia's markers next to a clean checkout do not make it dirty.
    assert_eq!(
        checkout_content_digest(&checkout),
        format!("git-tree:{tree}")
    );

    fs::write(checkout.join("a.txt"), "edited").expect("edit");
    assert_eq!(checkout_content_digest(&checkout), hashed(&checkout));

    fs::write(checkout.join("a.txt"), "one").expect("restore");
    assert_eq!(
        checkout_content_digest(&checkout),
        format!("git-tree:{tree}")
    );

    fs::write(checkout.join("extra.txt"), "untracked").expect("extra");
    assert_eq!(checkout_content_digest(&checkout), hashed(&checkout));
}

#[test]
fn mirror_is_fetched_only_when_it_lacks_the_wanted_commit() {
    let scratch = TempDir::new("gaia-mirror-refresh");
    let repo = scratch.join("upstream");
    init_repo(&repo);
    fs::write(repo.join("a.txt"), "one").expect("file");
    let present = commit_all(&repo, "one");
    let mirror = scratch.join("mirror.git");
    let output = std::process::Command::new("git")
        .arg("clone")
        .arg("-q")
        .arg("--mirror")
        .arg(&repo)
        .arg(&mirror)
        .output()
        .expect("git clone --mirror");
    assert!(output.status.success());
    let absent = "0".repeat(40);
    assert!(is_full_commit_id(&present));
    assert!(!is_full_commit_id("main"));
    assert!(!is_full_commit_id(&present[..39]));

    let fetches = Cell::new(0);
    refresh_mirror_with(
        &mirror,
        MirrorRefresh::IfMissing(&present),
        counting(&fetches),
    )
    .expect("present commit");
    assert_eq!(fetches.get(), 0, "a present commit must not fetch");

    refresh_mirror_with(
        &mirror,
        MirrorRefresh::IfMissing(&absent),
        counting(&fetches),
    )
    .expect("missing commit");
    assert_eq!(fetches.get(), 1, "a missing commit fetches once");

    refresh_mirror_with(&mirror, MirrorRefresh::Always, counting(&fetches)).expect("always");
    assert_eq!(fetches.get(), 2, "a branch or tag always fetches");

    // A failed fetch is an error for a ref, but only a warning for a commit
    // whose checkout will fail on its own anyway.
    let failing = || Err(SourceProviderError::backend_command("network down"));
    assert!(refresh_mirror_with(&mirror, MirrorRefresh::Always, failing).is_err());
    assert!(refresh_mirror_with(&mirror, MirrorRefresh::IfMissing(&absent), failing).is_ok());
}

#[test]
fn checkout_is_cloned_again_when_it_cannot_be_updated() {
    let scratch = TempDir::new("gaia-in-place-fallback");
    let repo = scratch.join("upstream");
    init_repo(&repo);
    fs::write(repo.join("a.txt"), "one").expect("file");
    commit_all(&repo, "one");
    let spec = spec_for(&scratch.join("build"));
    let source = source_for(&repo, None);
    run_source(&spec, &source);
    let checkout = materialized_dir(&spec, &source);

    // A checkout whose HEAD is gone cannot be updated: it is cloned again.
    fs::write(repo.join("a.txt"), "two").expect("file");
    let second = commit_all(&repo, "two");
    fs::remove_file(checkout.join(".git/HEAD")).expect("break checkout");
    run_source(&spec, &source);
    assert_eq!(
        fs::read_to_string(checkout.join("a.txt")).expect("file"),
        "two"
    );
    assert_eq!(git_head_commit(&checkout).as_deref(), Some(second.as_str()));

    // A checkout of another origin is cloned again, and gets this origin back.
    git(&checkout, &["remote", "set-url", "origin", "/nonexistent"]);
    fs::write(repo.join("a.txt"), "three").expect("file");
    commit_all(&repo, "three");
    run_source(&spec, &source);
    assert_eq!(
        fs::read_to_string(checkout.join("a.txt")).expect("file"),
        "three"
    );
    assert_eq!(
        git(&checkout, &["config", "--get", "remote.origin.url"]),
        repo.display().to_string()
    );
}

#[test]
fn a_locked_commit_missing_from_the_source_fails_the_source() {
    let scratch = TempDir::new("gaia-missing-lock");
    let repo = scratch.join("upstream");
    init_repo(&repo);
    fs::write(repo.join("a.txt"), "one").expect("file");
    commit_all(&repo, "one");
    let spec = spec_for(&scratch.join("build"));
    let source = source_for(&repo, Some("1".repeat(40)));
    let error = GitSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect_err("a missing locked commit must fail");
    assert!(error.message.contains("has no commit"), "{}", error.message);
}

#[test]
fn a_checkout_already_at_the_commit_is_restored_when_it_was_changed() {
    let scratch = TempDir::new("gaia-up-to-date");
    let repo = scratch.join("upstream");
    init_repo(&repo);
    fs::write(repo.join("a.txt"), "one").expect("file");
    commit_all(&repo, "one");
    let spec = spec_for(&scratch.join("build"));
    let mut source = source_for(&repo, None);
    run_source(&spec, &source);
    let checkout = materialized_dir(&spec, &source);

    // Same commit and nothing changed: the checkout is kept as it is.
    fs::write(checkout.join(".git/gaia-test-marker"), "kept").expect("marker");
    run_source(&spec, &source);
    assert!(checkout.join(".git/gaia-test-marker").exists());

    // A local edit and an ignored file go even though HEAD matches.
    fs::write(checkout.join("a.txt"), "edited").expect("edit");
    fs::write(checkout.join(".gitignore"), "*.log\n").expect("gitignore");
    fs::write(checkout.join("ignored.log"), "ignored").expect("ignored");
    run_source(&spec, &source);
    assert_eq!(
        fs::read_to_string(checkout.join("a.txt")).expect("file"),
        "one"
    );
    assert!(!checkout.join("ignored.log").exists());

    // The subdir marker follows the source's configuration.
    if let SourceDefinition::Git(git) = &mut source.definition {
        git.subdir = Some("sub".into());
    }
    run_source(&spec, &source);
    assert_eq!(
        fs::read_to_string(checkout.join("selected-subdir.txt")).expect("subdir marker"),
        "sub"
    );
    if let SourceDefinition::Git(git) = &mut source.definition {
        git.subdir = None;
    }
    run_source(&spec, &source);
    assert!(!checkout.join("selected-subdir.txt").exists());
}

#[test]
fn remote_checkouts_share_their_mirror_and_fetch_only_what_they_need() {
    let scratch = TempDir::new("gaia-remote-mirror");
    let repo = scratch.join("upstream");
    init_repo(&repo);
    fs::write(repo.join("a.txt"), "one").expect("file");
    let first = commit_all(&repo, "one");
    let url = format!("file://{}", repo.display());
    let git_source = git_spec(url.clone(), Some(first.clone()));
    let spec = spec_for(&scratch.join("build"));
    let execution = SourceExecutionContext {
        workspace_root: scratch.join(""),
        docker: None,
    };
    let run = GitRun {
        execution: &execution,
        policy: SourceCommandPolicy {
            attempts: 1,
            retry_backoff_ms: 0,
            retry_backoff_strategy: RetryBackoffStrategySpec::Fixed,
            timeout_seconds: 60,
            output_retention: process_output_retention(&spec),
        },
        log_sink: None,
        cancel_check: None,
    };
    let checkout = scratch.join("checkout");

    // The locked commit comes from a new mirror, shared rather than copied.
    let mirror = ensure_remote_git_cache(&git_source, &run, MirrorRefresh::IfMissing(&first))
        .expect("mirror");
    assert_eq!(
        mirror,
        remote_git_mirror_dir(&scratch.join(GIT_MIRROR_CACHE_DIR), &url)
    );
    assert!(mirror.join("HEAD").is_file(), "mirror not created");
    checkout_git_commit(
        &git_source,
        &checkout,
        &CheckoutSource::Mirror(mirror.clone()),
        &first,
        &run,
    )
    .expect("locked checkout");
    assert_eq!(git_head_commit(&checkout).as_deref(), Some(first.as_str()));
    assert!(
        checkout.join(".git/objects/info/alternates").is_file(),
        "checkout copied the mirror's objects instead of sharing them"
    );
    assert_eq!(
        git(&checkout, &["config", "--get", "remote.origin.url"]),
        repo.display().to_string(),
        "origin must be the configured repository, not the mirror"
    );

    // With the mirror unreachable, a branch fails instead of resolving from
    // stale refs: it always fetches.
    git(&mirror, &["remote", "set-url", "origin", "/nonexistent"]);
    // A full commit id the mirror holds needs no fetch, so it resolves.
    let mut pinned = git_spec(url.clone(), None);
    pinned.branch = None;
    pinned.rev = Some(first.clone());
    let remote = resolve_remote_checkout(&pinned, &run).expect("a held commit needs no fetch");
    assert_eq!(remote.target, first);
    let error = resolve_remote_checkout(&git_spec(url.clone(), None), &run)
        .err()
        .expect("a branch must fetch its mirror");
    assert!(
        error.message.contains("refresh remote git source cache"),
        "{}",
        error.message
    );

    // Reachable again: the branch resolves to the new tip, from the mirror.
    git(&mirror, &["remote", "set-url", "origin", &url]);
    fs::write(repo.join("a.txt"), "two").expect("file");
    let second = commit_all(&repo, "two");
    let remote = resolve_remote_checkout(&git_spec(url.clone(), None), &run).expect("branch");
    assert_eq!(remote.target, second);
    assert_eq!(remote.mirror, mirror);
    assert!(remote.refs.contains("refs/heads/main"), "{}", remote.refs);
    checkout_git_commit(
        &git_spec(url.clone(), None),
        &checkout,
        &CheckoutSource::Mirror(remote.mirror),
        &remote.target,
        &run,
    )
    .expect("branch checkout");
    assert_eq!(git_head_commit(&checkout).as_deref(), Some(second.as_str()));
    assert_eq!(
        fs::read_to_string(checkout.join("a.txt")).expect("file"),
        "two"
    );
}
