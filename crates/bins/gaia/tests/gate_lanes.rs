//! `scripts/gate-lanes.py`: what the gate has to test. Run on a small
//! workspace of its own (crate `low`, crate `high` that depends on it, and
//! `lone`), in a git repository of its own, with its own pass records.

use std::path::{Path, PathBuf};
use std::process::Command;

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scripts/gate-lanes.py")
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

#[test]
fn the_gate_tests_what_a_change_can_reach_and_nothing_a_passed_tree_holds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = dir.path().join("repo");
    let records = dir.path().join("records");
    write(
        &repo.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    );
    for (name, deps) in [
        ("low", ""),
        ("high", "low = { path = \"../low\" }\n"),
        ("lone", ""),
    ] {
        write(
            &repo.join(format!("crates/{name}/Cargo.toml")),
            &format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{deps}"
            ),
        );
        write(&repo.join(format!("crates/{name}/src/lib.rs")), "\n");
    }
    write(
        &repo.join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"high\"\nversion = \"0.1.0\"\ndependencies = [\n \"low\",\n]\n\n[[package]]\nname = \"lone\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"low\"\nversion = \"0.1.0\"\n",
    );
    write(&repo.join("docs/a.md"), "a\n");
    write(
        &repo.join("scripts/gate-lanes.py"),
        &std::fs::read_to_string(script()).expect("read the script"),
    );
    write(&repo.join(".gitignore"), "target/\n");
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "one"]);
    let run = |arg: &str| {
        let out = Command::new("python3")
            .arg(repo.join("scripts/gate-lanes.py"))
            .arg(arg)
            .current_dir(&repo)
            .env("GAIA_GATE_RECORDS", &records)
            .env("CARGO_TARGET_DIR", dir.path().join("target"))
            .output()
            .expect("runs");
        assert!(out.status.success(), "{out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    // Nothing passed yet: everything.
    assert!(run("plan").starts_with("full no passed code tree"));
    run("passed");
    // The tree that passed: nothing to run.
    assert!(run("plan").starts_with("none "));

    // A change to `low` reaches `high`, not `lone`.
    write(&repo.join("crates/low/src/lib.rs"), "pub fn x() {}\n");
    let plan = run("plan");
    let words: Vec<&str> = plan.split_whitespace().collect();
    assert_eq!(words[0], "lanes", "{plan}");
    assert_eq!(&words[2..], ["high", "low"], "{plan}");

    // Committed, it is still measured from the passed ancestor.
    git(&repo, &["commit", "-q", "-am", "two"]);
    let plan = run("plan");
    assert_eq!(
        &plan.split_whitespace().collect::<Vec<_>>()[2..],
        ["high", "low"]
    );

    // A docs change alone: nothing new to test.
    git(&repo, &["reset", "-q", "--hard", "HEAD~1"]);
    write(&repo.join("docs/a.md"), "b\n");
    git(&repo, &["commit", "-q", "-am", "docs"]);
    assert!(run("plan").starts_with("none "), "docs only");
    // A workflow alone: the gate does not read it.
    write(&repo.join(".github/workflows/next.yml"), "on: push\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "workflow"]);
    assert!(run("plan").starts_with("none "), "a workflow only");

    // Anything outside a crate: everything.
    write(&repo.join("rust-toolchain.toml"), "[toolchain]\n");
    assert!(
        run("plan").starts_with("full rust-toolchain.toml is outside every crate"),
        "a top-level file"
    );
    // A lock that does not read: everything.
    write(&repo.join("Cargo.lock"), "version = 4\n");
    assert!(run("plan").starts_with("full "), "a broken lock");
}
