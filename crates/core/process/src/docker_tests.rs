//! A command stopped early must not leave its docker container running.
use super::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// A fake `docker`: logs its arguments, and for `run` writes the container
/// id to the `--cidfile` path, then either sleeps (a long build) or exits.
fn fake_docker(dir: &Path, run_body: &str) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    fs::create_dir_all(dir).expect("dir");
    let log = dir.join("calls.log");
    let script = format!(
        "#!/bin/sh\necho \"$@\" >> '{}'\nif [ \"$1\" = run ]; then\n  \
         while [ \"$1\" != --cidfile ]; do shift; done\n  \
         echo fake-container-id > \"$2\"\n  {run_body}\nfi\nexit 0\n",
        log.display()
    );
    // Write then rename, so no fork can inherit an open write handle.
    let staged = dir.join("docker.tmp");
    fs::write(&staged, script).expect("script");
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o755)).expect("chmod");
    let docker = dir.join("docker");
    fs::rename(&staged, &docker).expect("install script");
    (docker, log)
}

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "gaia-docker-cleanup-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ))
}

fn docker_run(docker: &Path, cidfile: &Path) -> Command {
    let mut command = Command::new(docker);
    command
        .args(["run", "--rm", "--init", "--cidfile"])
        .arg(cidfile)
        .args(["image", "make"]);
    command
}

#[test]
fn timed_out_docker_run_removes_its_container() {
    let dir = temp_dir("timeout");
    let (docker, log) = fake_docker(&dir, "exec sleep 30");
    let cidfile = dir.join("container.cid");

    let error = run_command_with_timeout(
        &mut docker_run(&docker, &cidfile),
        Duration::from_secs(1),
        "docker-timeout-test",
        None,
        None,
    )
    .expect_err("the command should time out");

    assert_eq!(error.kind, ProcessRunErrorKind::Timeout);
    let calls = fs::read_to_string(&log).expect("docker calls");
    assert!(
        calls
            .lines()
            .any(|line| line == "rm --force fake-container-id"),
        "{calls}"
    );
    assert!(!cidfile.exists(), "container id file should be removed");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn cancelled_docker_run_removes_its_container() {
    let dir = temp_dir("cancel");
    let (docker, log) = fake_docker(&dir, "exec sleep 30");
    let cidfile = dir.join("container.cid");
    let started = std::time::Instant::now();
    let cancel: ProcessCancelCheck =
        std::sync::Arc::new(move || started.elapsed() > Duration::from_millis(500));

    let error = run_command_with_timeout(
        &mut docker_run(&docker, &cidfile),
        Duration::from_secs(30),
        "docker-cancel-test",
        None,
        Some(cancel),
    )
    .expect_err("the command should be cancelled");

    assert_eq!(error.kind, ProcessRunErrorKind::Cancelled);
    let calls = fs::read_to_string(&log).expect("docker calls");
    assert!(calls.contains("rm --force fake-container-id"), "{calls}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn finished_docker_run_only_discards_the_id_file() {
    let dir = temp_dir("finished");
    let (docker, log) = fake_docker(&dir, "exit 0");
    let cidfile = dir.join("container.cid");

    run_command_with_timeout(
        &mut docker_run(&docker, &cidfile),
        Duration::from_secs(30),
        "docker-finished-test",
        None,
        None,
    )
    .expect("the command should succeed");

    let calls = fs::read_to_string(&log).expect("docker calls");
    assert!(!calls.contains("rm --force"), "{calls}");
    assert!(!cidfile.exists(), "container id file should be removed");
    let _ = fs::remove_dir_all(dir);
}
