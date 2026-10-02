//! Git checkouts used while resolving config (`imports` from a git source).
//!
//! Unlike source materialization this runs before a build spec exists, so it
//! takes plain paths instead of an execution context. Remote repositories go
//! through the same per-repo mirror under `.gaia/cache/git` that git sources
//! use; local repositories (`file://` URLs or paths) are cloned directly.

use super::*;

/// Workspace-relative directory holding config-import checkouts.
pub const IMPORT_SOURCE_CACHE_DIR: &str = ".gaia/cache/import-sources";

/// Makes sure `dest` holds a detached checkout of `rev` from `repo` and
/// returns the checked-out commit. An existing checkout is reused as is, so
/// nothing is fetched once a revision has been checked out.
pub fn ensure_git_revision_checkout(
    workspace_root: &Path,
    repo: &str,
    rev: &str,
    dest: &Path,
    timeout: Duration,
) -> Result<String, String> {
    if dest.join(".git").exists() {
        return git_head_commit(dest)
            .ok_or_else(|| format!("checkout '{}' has no readable HEAD", dest.display()));
    }
    let parent = dest
        .parent()
        .ok_or_else(|| format!("checkout path '{}' has no parent", dest.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create '{}': {error}", parent.display()))?;
    let clone_from = if is_local_git_repo(repo) {
        repo.strip_prefix("file://").unwrap_or(repo).to_string()
    } else {
        ensure_import_mirror(workspace_root, repo, rev, timeout)?
            .display()
            .to_string()
    };

    let staging = parent.join(format!(
        ".{}.tmp-{}",
        dest.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&staging);
    let result = clone_and_checkout(&clone_from, rev, &staging, timeout);
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if let Err(error) = fs::rename(&staging, dest) {
        let _ = fs::remove_dir_all(&staging);
        // Another process may have finished the same checkout first.
        if !dest.join(".git").exists() {
            return Err(format!(
                "failed to move checkout into '{}': {error}",
                dest.display()
            ));
        }
    }
    git_head_commit(dest)
        .ok_or_else(|| format!("checkout '{}' has no readable HEAD", dest.display()))
}

fn clone_and_checkout(
    clone_from: &str,
    rev: &str,
    staging: &Path,
    timeout: Duration,
) -> Result<(), String> {
    let mut clone = git_command();
    clone
        .arg("clone")
        .arg("--quiet")
        .arg("--no-checkout")
        .arg(clone_from)
        .arg(staging);
    run_git(clone, timeout, &format!("git clone '{clone_from}'"))?;
    let mut checkout = git_command();
    checkout
        .arg("-C")
        .arg(staging)
        .arg("checkout")
        .arg("--quiet")
        .arg("--detach")
        .arg(rev);
    run_git(checkout, timeout, &format!("git checkout '{rev}'"))
}

fn ensure_import_mirror(
    workspace_root: &Path,
    repo: &str,
    rev: &str,
    timeout: Duration,
) -> Result<PathBuf, String> {
    let mirror = workspace_root
        .join(GIT_MIRROR_CACHE_DIR)
        .join(remote_git_mirror_dir_name(repo));
    if mirror.join("HEAD").is_file() {
        let mut has_rev = git_command();
        has_rev
            .arg("-C")
            .arg(&mirror)
            .arg("cat-file")
            .arg("-e")
            .arg(format!("{rev}^{{commit}}"));
        if run_git(has_rev, timeout, "git cat-file").is_ok() {
            return Ok(mirror);
        }
        let mut fetch = git_command();
        fetch
            .arg("-C")
            .arg(&mirror)
            .arg("fetch")
            .arg("--quiet")
            .arg("--prune")
            .arg("origin");
        run_git(fetch, timeout, &format!("git fetch '{repo}'"))?;
        return Ok(mirror);
    }
    if let Some(parent) = mirror.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create '{}': {error}", parent.display()))?;
    }
    let mut clone = git_command();
    clone
        .arg("clone")
        .arg("--quiet")
        .arg("--mirror")
        .arg(repo)
        .arg(&mirror);
    run_git(clone, timeout, &format!("git clone --mirror '{repo}'"))?;
    Ok(mirror)
}

fn run_git(mut command: Command, timeout: Duration, label: &str) -> Result<(), String> {
    let output = gaia_process::run_command_with_timeout(&mut command, timeout, label, None, None)
        .map_err(|error| error.message)?
        .output;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{label} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}
