//! Node `gaia.source.git`: a git commit, as a stored tree of its checkout.
//!
//! Inputs: the store, the `git` program (pinned by path), the repository
//! URL and the commit the lock resolved. Output: a [`Tree`] of the checkout
//! without `.git`.
//!
//! - **Key:** the commit, and nothing else. The same commit from any URL or
//!   mirror is the same tree.
//! - **Shared object cache:** objects are fetched into one bare repository
//!   per URL in `git/` next to the store, shared by every product that uses
//!   the store. A commit already there is not fetched again.
//! - **Exactly the commit:** a shallow fetch of that one commit, no tags,
//!   no refs written. A commit the remote lacks fails, naming the URL.
//! - **Submodules are declared, never implicit:** a submodule's path is
//!   left empty, as git leaves it before `submodule update`. A submodule is
//!   its own source in a description, checked out by its own node and put
//!   in place by a file tree step.
//! - **Modes:** files are `0644`, or `0755` when git marks them executable;
//!   directories `0755`. The tree's digest does not depend on the umask.

use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store, Tool, Work};
use gaia_types::Tree;
use sha2::{Digest as _, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The node's id, and the step name in its errors and logs.
pub const NAME: &str = "gaia.source.git";

/// The shared object cache for `store`: `git` next to its root.
pub fn cache_dir(store: &Store) -> PathBuf {
    store.root().with_file_name("git")
}

/// The bare repository in `cache` that holds `url`'s objects.
pub fn cache_repository(cache: &Path, url: &str) -> PathBuf {
    let digest = Sha256::digest(url.as_bytes());
    let hex: String = digest[..12].iter().map(|b| format!("{b:02x}")).collect();
    cache.join(format!("{hex}.git"))
}

/// Checks out `commit` of `url` into the store.
#[node(id = "gaia.source.git", inputs(store, git, url, commit), outputs(tree))]
pub fn source(
    store: &str,
    git: &str,
    url: &str,
    commit: &str,
    cx: &Ctx,
) -> Result<Tree, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let commit = commit.trim();
    if !is_commit(commit) {
        return Err(StepError::body(
            NAME,
            format!("{url}: `{commit}` is not a full commit id; the lock gives one"),
        ));
    }
    let key = KeyBuilder::new(NAME, 1).config("commit", commit).finish();
    let cache = cache_repository(&cache_dir(&store), url);
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("tree", Path::new("tree"))],
        |work| {
            fetch(work, git, url, commit, &cache)?;
            let tree = work.dir().join("tree");
            fs::create_dir(&tree).map_err(|e| StepError::body(NAME, e))?;
            let index = work.dir().join("index");
            let in_cache = |args: &[&str]| {
                Tool::new(git)
                    .arg("--git-dir")
                    .arg(&cache)
                    .arg("--work-tree")
                    .arg(&tree)
                    .args(args)
                    .env("PATH", "/usr/bin:/bin")
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .env("GIT_CONFIG_GLOBAL", "/dev/null")
                    .env("GIT_INDEX_FILE", &index)
            };
            // This step's own index, so steps sharing the cache never share one.
            work.run(&in_cache(&["read-tree", commit]))?;
            work.run(&in_cache(&["checkout-index", "--all", "--force"]))?;
            fs::remove_file(&index).map_err(|e| StepError::body(NAME, e))?;
            normalize_modes(&tree).map_err(|e| StepError::body(NAME, e))
        },
    )?;
    Ok(Tree {
        object: made.outputs["tree"].into(),
        entries: Vec::new(),
    })
}

/// Fetches `commit` from `url` into the cache, unless it is there.
fn fetch(
    work: &mut Work<'_>,
    git: &str,
    url: &str,
    commit: &str,
    cache: &Path,
) -> Result<(), StepError> {
    fs::create_dir_all(cache).map_err(|e| StepError::body(NAME, e))?;
    // One fetch into a cache repository at a time: git's shallow file takes
    // one writer.
    let lock =
        fs::File::create(cache.with_extension("lock")).map_err(|e| StepError::body(NAME, e))?;
    lock.lock().map_err(|e| StepError::body(NAME, e))?;
    let git_in = |args: &[&str]| {
        Tool::new(git)
            .arg("--git-dir")
            .arg(cache)
            .args(args)
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", work_home(cache))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
    };
    if !cache.join("HEAD").exists() {
        work.run(&git_in(&["init", "--quiet", "--bare"]))?;
    }
    if work
        .run(&git_in(&[
            "cat-file",
            "-e",
            &format!("{commit}^{{commit}}"),
        ]))
        .is_ok()
    {
        return Ok(());
    }
    work.run(&git_in(&[
        "fetch",
        "--quiet",
        "--no-tags",
        "--no-write-fetch-head",
        "--no-recurse-submodules",
        "--depth",
        "1",
        url,
        commit,
    ]))
    .map_err(|error| {
        let tail = error.stderr_tail().join("\n");
        let mut why = StepError::body(NAME, format!("{url}: cannot fetch commit {commit}: {tail}"));
        if let Some(log) = error.log() {
            why = why.with_log(log.to_path_buf());
        }
        why
    })?;
    // A shallow fetch of a commit that is not the one asked for (a remote
    // that ignored the want) is not accepted.
    work.run(&git_in(&[
        "cat-file",
        "-e",
        &format!("{commit}^{{commit}}"),
    ]))
    .map_err(|_| {
        StepError::body(
            NAME,
            format!("{url}: the fetch did not bring commit {commit}"),
        )
    })?;
    Ok(())
}

/// Git wants a home; the cache's parent is one nothing else reads.
fn work_home(cache: &Path) -> PathBuf {
    cache.parent().unwrap_or(cache).to_path_buf()
}

/// A full SHA-1 or SHA-256 commit id.
fn is_commit(text: &str) -> bool {
    matches!(text.len(), 40 | 64)
        && text
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Files 0644 or 0755 (any execute bit), directories 0755.
fn normalize_modes(dir: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
            normalize_modes(&path)?;
        } else {
            let mode = entry.metadata()?.permissions().mode();
            let mode = if mode & 0o111 != 0 { 0o755 } else { 0o644 };
            fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
        }
    }
    Ok(())
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&SOURCE_NODE];
