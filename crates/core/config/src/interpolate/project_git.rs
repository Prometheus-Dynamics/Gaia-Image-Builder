//! `${project.commit}`, `${project.commit_short}` (its first 8 characters),
//! `${project.commit_count}` (the number of commits HEAD has, which only
//! grows along a branch, so package versions order) and
//! `${project.describe}`: the git identity of the
//! repository holding the build file, so image versions can be unique per
//! build (`version = "1.4.0+${project.describe}"`).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use crate::source_tokens::short_commit;

/// How long a repository's identity is reused when only its working tree
/// changed. A commit or a branch move changes [`repo_stamp`] and is seen at
/// once; editing a tracked file changes neither, so `-dirty` can lag by this
/// much in a long-running TUI.
const DIRTY_TTL: Duration = Duration::from_secs(10);

#[derive(Clone, Default)]
struct ProjectGit {
    commit: Option<String>,
    describe: Option<String>,
    commit_count: Option<String>,
}

struct CachedIdentity {
    stamp: String,
    made: Instant,
    identity: ProjectGit,
}

/// Memoized per directory: interpolation resolves the same token many times.
/// An entry is reused while the repository's [`repo_stamp`] is unchanged and
/// it is younger than [`DIRTY_TTL`].
fn project_git(dir: &Path) -> ProjectGit {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedIdentity>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let stamp = repo_stamp(dir);
    if let Some(stamp) = &stamp
        && let Some(known) = cache.lock().ok().and_then(|cache| {
            cache
                .get(dir)
                .filter(|entry| &entry.stamp == stamp && entry.made.elapsed() < DIRTY_TTL)
                .map(|entry| entry.identity.clone())
        })
    {
        return known;
    }
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|value| !value.is_empty())
    };
    let commit = git(&["rev-parse", "HEAD"]).map(|head| {
        let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some();
        if dirty { format!("{head}-dirty") } else { head }
    });
    let identity = ProjectGit {
        describe: commit
            .as_ref()
            .and_then(|_| git(&["describe", "--tags", "--always", "--dirty"])),
        commit_count: commit
            .as_ref()
            .and_then(|_| git(&["rev-list", "--count", "HEAD"])),
        commit,
    };
    if let (Some(stamp), Ok(mut cache)) = (stamp, cache.lock()) {
        cache.insert(
            dir.to_path_buf(),
            CachedIdentity {
                stamp,
                made: Instant::now(),
                identity: identity.clone(),
            },
        );
    }
    identity
}

/// Identifies the commit a repository holding `dir` is at: its HEAD, the
/// branch HEAD names (loose or packed), and its index. `None` outside a
/// repository. Changes whenever a commit is made, a branch moves, or the index
/// is rewritten (staging, checkout, reset).
pub(super) fn repo_stamp(dir: &Path) -> Option<String> {
    let git_dir = git_dir(dir)?;
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let mut parts = vec![
        format!("head={}", head.trim()),
        format!("mtime:{}", mtime_nanos(&git_dir.join("HEAD"))),
        format!("packed:{}", mtime_nanos(&git_dir.join("packed-refs"))),
        format!("index:{}", mtime_nanos(&git_dir.join("index"))),
    ];
    if let Some(reference) = head.trim().strip_prefix("ref: ") {
        parts.push(format!("ref:{}", mtime_nanos(&git_dir.join(reference))));
    }
    Some(parts.join("|"))
}

/// The git directory of the repository holding `dir`, following a `.git`
/// file (a linked worktree) to its target.
fn git_dir(dir: &Path) -> Option<PathBuf> {
    let dot_git = dir
        .ancestors()
        .map(|ancestor| ancestor.join(".git"))
        .find(|candidate| candidate.exists())?;
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let pointer = fs::read_to_string(&dot_git).ok()?;
    let target = pointer.trim().strip_prefix("gitdir:")?.trim();
    let target = PathBuf::from(target);
    Some(if target.is_absolute() {
        target
    } else {
        dot_git.parent()?.join(target)
    })
}

/// Modification time of `path` in nanoseconds, or `-` when it is absent.
fn mtime_nanos(path: &Path) -> String {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or_else(
            || "-".to_string(),
            |duration| duration.as_nanos().to_string(),
        )
}

/// `Some(value)` for a `project.commit` / `project.describe` token when the
/// build file is inside a git repository; `None` leaves it unresolved.
pub(super) fn resolve(token: &str, build_file_dir: &str) -> Option<String> {
    let field = match token {
        "project.commit" => |git: ProjectGit| git.commit,
        "project.commit_short" => |git: ProjectGit| git.commit.map(|commit| short_commit(&commit)),
        "project.describe" => |git: ProjectGit| git.describe,
        "project.commit_count" => |git: ProjectGit| git.commit_count,
        _ => return None,
    };
    if build_file_dir.is_empty() {
        return None;
    }
    field(project_git(Path::new(build_file_dir)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_testkit::TempDir;

    /// Runs `git` in `dir` with a fixed author and committer date.
    fn git(dir: &Path, args: &[&str], date: &str) {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=Gaia Test",
                "-c",
                "user.email=gaia-test@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn project_commit_follows_new_commits_and_branch_moves() {
        let scratch = TempDir::new("gaia-config-project-git");
        let repo = scratch.join("repo");
        fs::create_dir_all(&repo).expect("repo dir");
        git(&repo, &["init", "-b", "main"], "@1000000000 +0000");
        git(
            &repo,
            &["commit", "--allow-empty", "-m", "one"],
            "@1000000000 +0000",
        );
        let dir = repo.display().to_string();
        let first = resolve("project.commit", &dir).expect("first commit");

        git(
            &repo,
            &["commit", "--allow-empty", "-m", "two"],
            "@1000000100 +0000",
        );
        let second = resolve("project.commit", &dir).expect("second commit");
        assert_ne!(first, second, "a new commit is seen, not the memoized one");

        git(
            &repo,
            &["checkout", "-q", "-b", "other"],
            "@1000000100 +0000",
        );
        git(
            &repo,
            &["commit", "--allow-empty", "-m", "three"],
            "@1000000200 +0000",
        );
        let third = resolve("project.commit", &dir).expect("third commit");
        assert_ne!(second, third, "a commit on another branch is seen");
    }
}
