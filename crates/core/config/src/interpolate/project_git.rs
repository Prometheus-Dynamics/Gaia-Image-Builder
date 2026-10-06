//! `${project.commit}` and `${project.describe}`: the git identity of the
//! repository holding the build file, so image versions can be unique per
//! build (`version = "1.4.0+${project.describe}"`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Default)]
struct ProjectGit {
    commit: Option<String>,
    describe: Option<String>,
}

/// Memoized per directory: interpolation resolves the same token many times.
fn project_git(dir: &Path) -> ProjectGit {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, ProjectGit>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(known) = cache.lock().ok().and_then(|cache| cache.get(dir).cloned()) {
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
        commit,
    };
    if let Ok(mut cache) = cache.lock() {
        cache.insert(dir.to_path_buf(), identity.clone());
    }
    identity
}

/// `Some(value)` for a `project.commit` / `project.describe` token when the
/// build file is inside a git repository; `None` leaves it unresolved.
pub(super) fn resolve(token: &str, build_file_dir: &str) -> Option<String> {
    let field = match token {
        "project.commit" => |git: ProjectGit| git.commit,
        "project.describe" => |git: ProjectGit| git.describe,
        _ => return None,
    };
    if build_file_dir.is_empty() {
        return None;
    }
    field(project_git(Path::new(build_file_dir)))
}
