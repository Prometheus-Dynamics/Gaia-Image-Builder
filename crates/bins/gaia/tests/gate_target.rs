//! `scripts/gate-target.sh`: a cargo target directory records the worktree
//! that uses it, and the gate refuses one that belongs to another.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("the repository")
}

fn run(target: Option<&Path>) -> (Option<i32>, String) {
    let mut command = Command::new(repo().join("scripts/gate-target.sh"));
    command.env_remove("CARGO_TARGET_DIR");
    if let Some(target) = target {
        command.env("CARGO_TARGET_DIR", target);
    }
    let out = command.output().expect("runs");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_target_directory_is_one_worktrees() {
    let dir = tempfile::tempdir().expect("tempdir");
    let here = repo().display().to_string();

    // A new directory: made, and this worktree recorded, without a word.
    let target = dir.path().join("new");
    let (code, err) = run(Some(&target));
    assert_eq!((code, err.as_str()), (Some(0), ""));
    let record = target.join(".gaia-worktree");
    assert_eq!(
        std::fs::read_to_string(&record).expect("record"),
        format!("{here}\n")
    );
    // Again, from the same worktree: allowed, the record as it was.
    let (code, err) = run(Some(&target));
    assert_eq!((code, err.as_str()), (Some(0), ""));

    // Another worktree's: refused, naming both, and the record is not changed.
    std::fs::write(&record, "/somewhere/else/gaia\n").expect("write");
    let (code, err) = run(Some(&target));
    assert_eq!(code, Some(1), "{err}");
    assert!(
        err.contains("belongs to another worktree")
            && err.contains("its worktree: /somewhere/else/gaia")
            && err.contains(&format!("this one:     {here}")),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(&record).expect("record"),
        "/somewhere/else/gaia\n"
    );

    // Not empty and with no record: this worktree is recorded, and it says
    // that what is there is of unknown origin.
    let old = dir.path().join("old");
    std::fs::create_dir_all(old.join("debug")).expect("mkdir");
    let (code, err) = run(Some(&old));
    assert_eq!(code, Some(0), "{err}");
    assert!(
        err.contains("has no worktree on record and is not empty"),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(old.join(".gaia-worktree")).expect("record"),
        format!("{here}\n")
    );

    // No CARGO_TARGET_DIR: the worktree's own `target`, nothing to check.
    let (code, err) = run(None);
    assert_eq!((code, err.as_str()), (Some(0), ""));
}
