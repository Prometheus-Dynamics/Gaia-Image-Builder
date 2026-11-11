//! The git source node against a local bare repository made here.

use gaia_source_git::{cache_dir, source};
use gaia_step::{Ctx, StepError, step_records};
use gaia_store::{Digest, Store, tree_digest};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const GIT: &str = "/usr/bin/git";

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(GIT)
        .args(args)
        .current_dir(dir)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@localhost")
        .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@localhost")
        .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout)
        .expect("utf-8")
        .trim()
        .to_string()
}

/// A bare repository with two commits. The first has a file, an executable,
/// a file in a directory, a symlink and a submodule entry.
struct Remote {
    _dir: tempfile::TempDir,
    url: String,
    bare: PathBuf,
    first: String,
    second: String,
}

fn remote() -> Remote {
    let dir = tempfile::tempdir().expect("tempdir");
    let work = dir.path().join("work");
    std::fs::create_dir_all(work.join("dir")).expect("mkdir");
    git(&work, &["init", "--quiet", "--initial-branch", "main"]);
    std::fs::write(work.join("a.txt"), "a\n").expect("write");
    std::fs::write(work.join("run.sh"), "#!/bin/sh\n").expect("write");
    std::fs::set_permissions(work.join("run.sh"), std::fs::Permissions::from_mode(0o775))
        .expect("chmod");
    std::fs::write(work.join("dir/b.txt"), "b\n").expect("write");
    std::os::unix::fs::symlink("a.txt", work.join("link")).expect("symlink");
    git(&work, &["add", "--all"]);
    // A submodule entry pointing at a commit no one fetches.
    git(
        &work,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            "160000,1111111111111111111111111111111111111111,sub",
        ],
    );
    git(&work, &["commit", "--quiet", "--message", "first"]);
    let first = git(&work, &["rev-parse", "HEAD"]);
    std::fs::write(work.join("a.txt"), "a, edited\n").expect("write");
    git(
        &work,
        &["commit", "--quiet", "--all", "--message", "second"],
    );
    let second = git(&work, &["rev-parse", "HEAD"]);
    let bare = dir.path().join("remote.git");
    git(
        dir.path(),
        &["clone", "--quiet", "--bare", "work", "remote.git"],
    );
    Remote {
        url: format!("file://{}", bare.display()),
        bare,
        _dir: dir,
        first,
        second,
    }
}

fn checkout(store: &Path, url: &str, commit: &str) -> Result<(Digest, PathBuf), StepError> {
    let tree = source(&store.display().to_string(), GIT, url, commit, &Ctx::new())?;
    let object = tree.object.object_ref().expect("a tree object");
    let store = Store::open(store).expect("store");
    Ok((object.digest, store.path(&object)))
}

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .expect("stat")
        .permissions()
        .mode()
        & 0o7777
}

#[test]
fn a_commit_checks_out_as_its_tree() {
    let remote = remote();
    let root = tempfile::tempdir().expect("tempdir");
    let store = root.path().join("store");
    let (digest, tree) = checkout(&store, &remote.url, &remote.first).expect("checks out");
    println!("first commit {} -> tree {digest}", remote.first);
    let read = |path: &str| std::fs::read_to_string(tree.join(path)).expect("read");
    assert_eq!(read("a.txt"), "a\n");
    assert_eq!(read("dir/b.txt"), "b\n");
    assert_eq!(
        std::fs::read_link(tree.join("link")).expect("link"),
        Path::new("a.txt")
    );
    assert_eq!(mode(&tree.join("a.txt")), 0o644);
    assert_eq!(mode(&tree.join("run.sh")), 0o755);
    assert_eq!(mode(&tree.join("dir")), 0o755);
    assert!(!tree.join(".git").exists());
    // The submodule is not fetched: its path holds nothing.
    let sub: Vec<_> = std::fs::read_dir(tree.join("sub"))
        .into_iter()
        .flatten()
        .collect();
    println!(
        "sub: exists {}, entries {}",
        tree.join("sub").exists(),
        sub.len()
    );
    assert!(sub.is_empty());
    assert_eq!(tree_digest(&tree).expect("digest"), digest);

    // The second commit is another tree.
    let (second, tree) = checkout(&store, &remote.url, &remote.second).expect("checks out");
    println!("second commit {} -> tree {second}", remote.second);
    assert_ne!(second, digest);
    assert_eq!(
        std::fs::read_to_string(tree.join("a.txt")).expect("read"),
        "a, edited\n"
    );
}

#[test]
fn the_same_commit_gives_the_same_digest_twice() {
    let remote = remote();
    let one = tempfile::tempdir().expect("tempdir");
    let two = tempfile::tempdir().expect("tempdir");
    let (a, _) = checkout(&one.path().join("store"), &remote.url, &remote.first).expect("one");
    let (b, _) = checkout(&two.path().join("store"), &remote.url, &remote.first).expect("two");
    println!("two fresh stores: {a} and {b}");
    assert_eq!(a, b);
    // Again in the first store: found, not made.
    let (c, _) = checkout(&one.path().join("store"), &remote.url, &remote.first).expect("again");
    assert_eq!(c, a);
    let store = Store::open(one.path().join("store")).expect("store");
    let made: Vec<bool> = step_records(&gaia_step::logs_dir(&store))
        .iter()
        .map(|record| record.made)
        .collect();
    println!("step record, made: {made:?}");
    assert_eq!(made, [true, false]);
}

#[test]
fn a_commit_in_the_cache_is_not_fetched_again() {
    let remote = remote();
    let root = tempfile::tempdir().expect("tempdir");
    let (a, _) = checkout(&root.path().join("store"), &remote.url, &remote.first).expect("first");
    let store = Store::open(root.path().join("store")).expect("store");
    let cache = cache_dir(&store);
    // A new store beside the same cache, and the remote gone.
    std::fs::remove_dir_all(&remote.bare).expect("rm remote");
    let (b, _) =
        checkout(&root.path().join("store2"), &remote.url, &remote.first).expect("from the cache");
    println!(
        "cache {}: same tree {a} without the remote",
        cache.display()
    );
    assert_eq!(a, b);
}

#[test]
fn a_missing_commit_fails_naming_the_url() {
    let remote = remote();
    let root = tempfile::tempdir().expect("tempdir");
    let missing = "2222222222222222222222222222222222222222";
    let error = checkout(&root.path().join("store"), &remote.url, missing).expect_err("fails");
    println!("missing commit: {error}");
    assert!(error.to_string().contains(&remote.url), "{error}");
    assert!(error.to_string().contains(missing), "{error}");
    // Nothing was stored.
    let store = Store::open(root.path().join("store")).expect("store");
    assert!(step_records(&gaia_step::logs_dir(&store)).is_empty());

    let error = checkout(&root.path().join("store"), &remote.url, "main").expect_err("fails");
    println!("not a commit id: {error}");
    assert!(
        error.to_string().contains("not a full commit id"),
        "{error}"
    );
}

#[test]
fn the_node_registers() {
    let mut registry = daedalus::prelude::Registry::new();
    registry
        .register_all(&gaia_source_git::NODES)
        .expect("registers");
}
