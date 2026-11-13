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
    let commit = match std::env::var("GAIA_COMMIT") {
        Ok(value) if !value.is_empty() => value,
        _ => git_commit(&manifest).unwrap_or_else(|| "unknown".to_string()),
    };
    println!("cargo:rustc-env=GAIA_COMMIT={commit}");
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
    watch_git_files(dir);
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
