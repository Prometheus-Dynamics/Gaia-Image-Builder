//! Records the commit `gaia` is built from, for `gaia --version`.
//!
//! `GAIA_COMMIT` set at build time wins, verbatim. Otherwise the commit is
//! `git rev-parse HEAD` in this crate's directory, with `-dirty` appended
//! when a tracked file has uncommitted changes. Without git, or outside a
//! checkout, it is `unknown`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=GAIA_COMMIT");
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let given = std::env::var("GAIA_COMMIT").ok();
    if given.as_deref().is_some_and(|value| !value.is_empty()) {
        // The commit is given, so the git files do not change it: watched,
        // every commit and index write ran this script again and rebuilt
        // gaia with the same result (the gate gives `gate`). A change of
        // GAIA_COMMIT, set or unset, still runs it again.
        println!("cargo:rerun-if-changed=build.rs");
    } else {
        watch_git_files(&manifest);
    }
    let commit = commit(&manifest, given);
    println!("cargo:rustc-env=GAIA_COMMIT={commit}");
}

/// The commit recorded for a build in `dir`, with `GAIA_COMMIT` as `given`.
/// The gate gives `gate`; a build without it records the checkout's HEAD.
pub(crate) fn commit(dir: &Path, given: Option<String>) -> String {
    match given {
        Some(value) if !value.is_empty() => value,
        _ => git_commit(dir).unwrap_or_else(|| "unknown".to_string()),
    }
}

/// Runs git in `dir` and returns its trimmed stdout, or `None` when git is
/// missing or the command fails.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_commit(dir: &Path) -> Option<String> {
    let head = git(dir, &["rev-parse", "HEAD"])?;
    if !is_full_hex(&head) {
        return None;
    }
    let dirty = !git(dir, &["status", "--porcelain", "--untracked-files=no"])?.is_empty();
    Some(if dirty { format!("{head}-dirty") } else { head })
}

/// Asks cargo to rerun this script when the files that change with HEAD
/// change. In a worktree `.git` is a file, so the paths come from git.
fn watch_git_files(dir: &Path) {
    let (Some(git_dir), Some(common_dir)) = (
        git(dir, &["rev-parse", "--git-dir"]),
        git(dir, &["rev-parse", "--git-common-dir"]),
    ) else {
        return;
    };
    let git_dir = absolute(dir, &git_dir);
    let common_dir = absolute(dir, &common_dir);
    let mut paths = vec![
        git_dir.join("HEAD"),
        git_dir.join("index"),
        common_dir.join("packed-refs"),
    ];
    if let Some(reference) = git(dir, &["symbolic-ref", "-q", "HEAD"]) {
        paths.push(common_dir.join(reference));
    }
    for path in paths {
        // Cargo reruns the script on every build when a watched path is
        // missing, so only paths that exist are watched.
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

fn absolute(dir: &Path, path: &str) -> PathBuf {
    let path = dir.join(path);
    std::fs::canonicalize(&path).unwrap_or(path)
}

fn is_full_hex(text: &str) -> bool {
    text.len() == 40
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
