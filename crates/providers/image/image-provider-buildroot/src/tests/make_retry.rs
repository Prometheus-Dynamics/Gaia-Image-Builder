use super::*;

/// A `make` stand-in that records each run in `count` and fails with `stderr`.
fn failing_make(dir: &Path, stderr: &str) -> PathBuf {
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let script = bin.join("make");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\necho run >> '{}'\necho '{stderr}' >&2\nexit 2\n",
            dir.join("count").display()
        ),
    )
    .expect("script");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("chmod");
    bin
}

fn runs(dir: &Path) -> usize {
    fs::read_to_string(dir.join("count"))
        .map(|text| text.lines().count())
        .unwrap_or(0)
}

fn make_command(bin: &Path) -> Command {
    let mut command = Command::new("make");
    command.env("PATH", format!("{}:/usr/bin:/bin", bin.display()));
    command
}

#[test]
fn make_stops_at_a_compile_error_however_many_attempts_are_set() {
    let scratch = TempDir::new("gaia-make-compile");
    let scratch = scratch.join("run");
    fs::create_dir_all(&scratch).expect("run dir");
    let bin = failing_make(&scratch, "error: no rule to make target 'x'");
    let execution = test_execution();
    let policy = make_policy(&ImageExecutionPolicy {
        make_retry_attempts: 3,
        ..ImageExecutionPolicy::default()
    });
    let result = run_command(
        make_command(&bin),
        "buildroot make",
        &execution,
        &policy,
        None,
        None,
    );
    assert!(result.is_err());
    assert_eq!(runs(&scratch), 1, "a compile error is not retried");
}

#[test]
fn make_retries_a_network_failure_up_to_its_attempts() {
    let scratch = TempDir::new("gaia-make-network");
    let scratch = scratch.join("run");
    fs::create_dir_all(&scratch).expect("run dir");
    let bin = failing_make(&scratch, "wget: unable to resolve host address 'x'");
    let execution = test_execution();
    let policy = make_policy(&ImageExecutionPolicy {
        make_retry_attempts: 3,
        ..ImageExecutionPolicy::default()
    });
    let result = run_command(
        make_command(&bin),
        "buildroot make",
        &execution,
        &policy,
        None,
        None,
    );
    assert!(result.is_err());
    assert_eq!(runs(&scratch), 3);
}

#[test]
fn make_policy_takes_its_own_timeout_and_attempts() {
    let base = ImageExecutionPolicy {
        timeout_seconds: 900,
        retry_attempts: 1,
        make_timeout_seconds: 0,
        make_retry_attempts: 2,
        ..ImageExecutionPolicy::default()
    };
    let make = make_policy(&base);
    assert_eq!(make.timeout_seconds, 0, "0 means no timeout for make");
    assert_eq!(make.retry_attempts, 2);
    assert_eq!(base.timeout_seconds, 900, "other commands keep theirs");
}

#[test]
fn network_failures_are_recognised_in_stderr_only() {
    assert!(looks_like_network_failure(
        b"curl: (6) Could not resolve host: x"
    ));
    assert!(looks_like_network_failure(b"Failed to download foo.tar.gz"));
    assert!(!looks_like_network_failure(
        b"error[E0308]: mismatched types"
    ));
    assert!(!looks_like_network_failure(
        b"gcc: fatal error: no such file"
    ));
}
