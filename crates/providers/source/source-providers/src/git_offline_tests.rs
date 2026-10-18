//! Offline git sources (`[execution] offline`): a remote checkout comes from
//! the cached mirror alone. The remote host in these tests does not resolve,
//! so any attempt to reach it would fail the test.

use super::*;
use crate::git_checkout_tests::{commit_all, git, git_spec, init_repo, spec_for};
use gaia_testkit::TempDir;

/// A remote URL that cannot be reached: offline runs must never try it.
const UNREACHABLE_REMOTE: &str = "https://offline-test.invalid/dep.git";

fn offline_spec(build_dir: &Path) -> ResolvedBuildSpec {
    let mut spec = spec_for(build_dir);
    spec.policy.execution.offline = true;
    spec
}

fn remote_source(locked: Option<String>) -> SourceSpec {
    SourceSpec::new(
        "offline-dep",
        SourceDefinition::Git(git_spec(UNREACHABLE_REMOTE.to_string(), locked)),
    )
}

/// A mirror of `upstream` in the place the provider keeps the mirror of
/// `UNREACHABLE_REMOTE` (under the workspace's `.gaia/cache/git`).
fn seed_mirror(scratch: &TempDir, upstream: &Path) -> PathBuf {
    let cache_dir = scratch.join(".gaia").join("cache").join("git");
    let mirror = remote_git_mirror_dir(&cache_dir, UNREACHABLE_REMOTE);
    fs::create_dir_all(&cache_dir).expect("cache dir");
    let output = std::process::Command::new("git")
        .arg("clone")
        .arg("-q")
        .arg("--mirror")
        .arg(upstream)
        .arg(&mirror)
        .output()
        .expect("git clone --mirror");
    assert!(output.status.success(), "mirror clone failed");
    mirror
}

fn upstream_repo(scratch: &TempDir) -> (PathBuf, String) {
    let repo = scratch.join("upstream");
    init_repo(&repo);
    fs::write(repo.join("a.txt"), "from the mirror").expect("file");
    let head = commit_all(&repo, "one");
    (repo, head)
}

#[test]
fn offline_remote_checkout_comes_from_the_mirror_without_the_network() {
    let scratch = TempDir::new("gaia-offline-git-mirror");
    let (upstream, _) = upstream_repo(&scratch);
    seed_mirror(&scratch, &upstream);
    let spec = offline_spec(&scratch.join("build"));
    let source = remote_source(None);

    GitSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect("offline checkout from the mirror");

    let checkout = materialized_dir(&spec, &source);
    assert_eq!(
        fs::read_to_string(checkout.join("a.txt")).expect("checked out file"),
        "from the mirror"
    );
}

#[test]
fn offline_locked_commit_missing_from_the_mirror_is_an_error_naming_it() {
    let scratch = TempDir::new("gaia-offline-git-missing");
    let (upstream, _) = upstream_repo(&scratch);
    seed_mirror(&scratch, &upstream);
    let spec = offline_spec(&scratch.join("build"));
    let missing = "1".repeat(40);
    let source = remote_source(Some(missing.clone()));

    let error = GitSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect_err("a commit the mirror lacks must fail offline");
    assert!(error.message.contains("offline:"), "{}", error.message);
    assert!(error.message.contains(&missing), "{}", error.message);
    assert!(error.message.contains("gaia prefetch"), "{}", error.message);
}

#[test]
fn offline_remote_without_a_mirror_says_to_prefetch() {
    let scratch = TempDir::new("gaia-offline-git-no-mirror");
    let spec = offline_spec(&scratch.join("build"));
    let source = remote_source(None);

    let error = GitSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect_err("no mirror, no network");
    assert!(error.message.contains("offline:"), "{}", error.message);
    assert!(error.message.contains("gaia prefetch"), "{}", error.message);
}

#[test]
fn offline_floating_branch_resolves_to_the_mirror_as_it_was_fetched() {
    let scratch = TempDir::new("gaia-offline-git-floating");
    let (upstream, first) = upstream_repo(&scratch);
    seed_mirror(&scratch, &upstream);
    // The upstream moves on; offline must keep the mirror's commit.
    fs::write(upstream.join("a.txt"), "moved on").expect("file");
    commit_all(&upstream, "two");
    let spec = offline_spec(&scratch.join("build"));
    let source = remote_source(None);

    GitSourceProvider
        .execute_source(&spec, &source, None, None)
        .expect("offline checkout");

    let checkout = materialized_dir(&spec, &source);
    assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), first);
}
