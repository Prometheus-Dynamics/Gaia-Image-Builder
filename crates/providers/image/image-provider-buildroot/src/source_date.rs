//! `SOURCE_DATE_EPOCH` for Buildroot's `make` and its scripts, so the
//! timestamps and archive metadata Buildroot writes are reproducible.
//!
//! The value is the caller's own `SOURCE_DATE_EPOCH` when its environment
//! sets one; otherwise the commit time of the workspace's git `HEAD`. A
//! commit time is stable for a commit (uncommitted edits do not change it)
//! and changes with each commit. Without a git workspace nothing is set and
//! Buildroot uses its own default.
//!
//! The value is an environment variable only: package cache keys do not
//! hash it. A package restored from the cache keeps the timestamps of the
//! epoch it was built with, so a tree built after a new commit can contain
//! cached packages from older epochs until those packages are rebuilt.
use super::*;
use std::sync::{Mutex, OnceLock};

/// The `SOURCE_DATE_EPOCH` to give `make`, when there is one.
pub(crate) fn source_date_epoch(spec: &ResolvedBuildSpec) -> Option<String> {
    env::var("SOURCE_DATE_EPOCH")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| workspace_head_epoch(Path::new(&spec.workspace.root_dir)))
}

/// The commit time of `root`'s `HEAD`, memoized: every `make` asks.
fn workspace_head_epoch(root: &Path) -> Option<String> {
    static CACHE: OnceLock<Mutex<BTreeMap<PathBuf, Option<String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Ok(cache) = cache.lock()
        && let Some(found) = cache.get(root)
    {
        return found.clone();
    }
    let epoch = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["log", "-1", "--format=%ct", "HEAD"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()));
    if let Ok(mut cache) = cache.lock() {
        cache.insert(root.to_path_buf(), epoch.clone());
    }
    epoch
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_workspace_head_commit_time_is_used_when_the_environment_has_none() {
        let root = std::env::temp_dir().join(format!("gaia-source-date-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("workspace");
        let git = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .env("GIT_AUTHOR_NAME", "gaia")
                .env("GIT_AUTHOR_EMAIL", "gaia@example.invalid")
                .env("GIT_COMMITTER_NAME", "gaia")
                .env("GIT_COMMITTER_EMAIL", "gaia@example.invalid")
                .env("GIT_COMMITTER_DATE", "1700000000 +0000")
                .env("GIT_AUTHOR_DATE", "1700000000 +0000")
                .status()
                .expect("git")
        };
        if !git(&["init", "-q"]).success() {
            let _ = fs::remove_dir_all(&root);
            return; // no git in this environment
        }
        fs::write(root.join("file"), "x").expect("file");
        assert!(git(&["add", "file"]).success());
        assert!(git(&["commit", "-q", "-m", "one"]).success());
        assert_eq!(workspace_head_epoch(&root).as_deref(), Some("1700000000"));
        let _ = fs::remove_dir_all(&root);
        // Not a repository (a separate path, so the memoized answer above
        // cannot answer for it): nothing to offer.
        let plain =
            std::env::temp_dir().join(format!("gaia-source-date-plain-{}", std::process::id()));
        let _ = fs::create_dir_all(&plain);
        assert_eq!(workspace_head_epoch(&plain), None);
        let _ = fs::remove_dir_all(&plain);
    }
}
