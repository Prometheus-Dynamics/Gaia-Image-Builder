//! `scripts/test-bound.py`: the gate's per-test bound. Its own self-test
//! (three cases on one CPU shared with spinners, judged by the time a test
//! could use, never by wall time), and what it does as a runner: a command
//! that ends keeps its exit code, and one that outlives its bound is stopped
//! with its whole process group and fails with 124, by name.

use std::path::{Path, PathBuf};
use std::process::Command;

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scripts/test-bound.py")
}

fn python() -> bool {
    Command::new("python3")
        .arg("-c")
        .arg("import os, sys; sys.exit(0 if hasattr(os, 'pidfd_open') else 1)")
        .status()
        .is_ok_and(|status| status.success())
}

#[test]
fn the_bound_counts_usable_time() {
    if !python() {
        return;
    }
    let out = Command::new(script())
        .arg("--self-test")
        .env_remove("GAIA_TEST_BOUND")
        .output()
        .expect("runs");
    let text = String::from_utf8_lossy(&out.stdout);
    println!("{text}{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{text}");
    assert!(!text.contains("FAIL"), "{text}");
}

#[test]
fn a_command_keeps_its_exit_code_and_a_hung_one_is_stopped_by_name() {
    if !python() {
        return;
    }
    let run = |bound: &str, code: &str| {
        Command::new(script())
            .args(["python3", "-c", code])
            .env("GAIA_TEST_BOUND", bound)
            .env_remove("GAIA_TEST_BOUND_SCALE")
            .output()
            .expect("runs")
    };
    // Ends by itself: its code, and nothing said.
    let out = run("30", "import sys; sys.exit(7)");
    assert_eq!(out.status.code(), Some(7));
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(run("30", "pass").status.code(), Some(0));

    // Outlives its bound, with a child of its own in the group: both are
    // stopped, and the failure names the command, the bound and the time.
    let dir = tempfile::tempdir().expect("tempdir");
    let mark = dir.path().join("child-alive");
    let code = format!(
        "import os, subprocess, sys, time\n\
         subprocess.Popen([sys.executable, '-c', 'import time\\nwhile True:\\n    open({mark:?}, \"w\").close(); time.sleep(0.1)'])\n\
         time.sleep(600)\n"
    );
    let out = run("2", &code);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(124), "{err}");
    assert!(
        err.contains("still running at its bound of 2 s of usable time")
            && err.contains("stopped it"),
        "{err}"
    );
    // The child is gone too: its mark is not written again.
    let _ = std::fs::remove_file(&mark);
    std::thread::sleep(std::time::Duration::from_millis(600));
    assert!(!mark.exists(), "the hung command's child is still running");
}
