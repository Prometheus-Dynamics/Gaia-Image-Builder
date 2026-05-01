use super::*;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

#[test]
fn command_description_captures_program_args_and_cwd() {
    let mut command = Command::new("gaia-test-tool");
    command
        .arg("--flag")
        .arg("value")
        .arg("--token")
        .arg("secret-token")
        .arg("PASSWORD=super-secret")
        .arg("https://user:secret@example.invalid/repo?access_token=abc&ok=1")
        .current_dir("/tmp");

    let description = command_description(&command);

    assert_eq!(description.program, "gaia-test-tool");
    assert_eq!(
        description.args,
        vec![
            "--flag",
            "value",
            "--token",
            "<redacted>",
            "PASSWORD=<redacted>",
            "https://<redacted>@example.invalid/repo?access_token=<redacted>&ok=1"
        ]
    );
    assert_eq!(description.cwd.as_deref(), Some("/tmp"));
}

#[test]
fn run_command_reports_cancellation() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancel_check: ProcessCancelCheck = {
        let cancelled = cancelled.clone();
        Arc::new(move || cancelled.load(Ordering::SeqCst))
    };
    let trigger = cancelled.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        trigger.store(true, Ordering::SeqCst);
    });

    let error = run_command_with_timeout(
        Command::new("bash")
            .arg("-lc")
            .arg("echo starting; sleep 5; echo finished"),
        Duration::from_secs(10),
        "cancel-test",
        None,
        Some(cancel_check),
    )
    .expect_err("command should be cancelled");

    assert_eq!(error.kind, ProcessRunErrorKind::Cancelled);
    assert!(error.message.contains("cancel-test cancelled"));
}

#[cfg(unix)]
#[test]
fn run_command_does_not_hang_when_child_leaves_inherited_stdio_open() {
    let start = Instant::now();
    let result = run_command_with_timeout(
        Command::new("bash")
            .arg("-lc")
            .arg("echo ready; (sleep 5) & exit 0"),
        Duration::from_secs(10),
        "inherited-stdio-test",
        None,
        None,
    )
    .expect("command should return even if a forked child keeps stdio open");

    assert!(start.elapsed() < Duration::from_secs(3));
    assert!(result.output.status.success());
    assert!(result.stdout_lines.iter().any(|line| line == "ready"));
}

#[cfg(unix)]
#[test]
fn run_command_timeout_kills_spawned_descendants() {
    let dir = unique_dir("process-tree-timeout");
    fs::create_dir_all(&dir).expect("create test dir");
    let marker = dir.join("descendant-survived");
    let script = format!(
        "printf 'spawned\\n'; (sleep 1; printf survived > '{}') & sleep 10",
        marker.display()
    );

    let error = run_command_with_timeout(
        Command::new("bash").arg("-lc").arg(script),
        Duration::from_millis(100),
        "process-tree-timeout-test",
        None,
        None,
    )
    .expect_err("command should time out");

    assert_eq!(error.kind, ProcessRunErrorKind::Timeout);
    thread::sleep(Duration::from_millis(1_300));
    assert!(!marker.exists(), "descendant survived process-tree kill");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn sleep_with_cancel_wakes_before_deadline() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancel_check: ProcessCancelCheck = {
        let cancelled = cancelled.clone();
        Arc::new(move || cancelled.load(Ordering::SeqCst))
    };
    let trigger = cancelled.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        trigger.store(true, Ordering::SeqCst);
    });

    let start = Instant::now();
    let completed = sleep_with_cancel(Duration::from_secs(5), Some(&cancel_check));

    assert!(!completed);
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn retry_backoff_duration_supports_fixed_and_exponential_strategies() {
    assert_eq!(
        retry_backoff_duration(ProcessRetryBackoffStrategy::Fixed, 25, 4),
        Duration::from_millis(25)
    );
    assert_eq!(
        retry_backoff_duration(ProcessRetryBackoffStrategy::Exponential, 25, 4),
        Duration::from_millis(200)
    );
}

#[test]
fn clone_command_preserves_program_args_cwd_and_env() {
    let cwd = std::env::current_dir().expect("cwd");
    let mut command = Command::new("echo");
    command
        .arg("hello")
        .current_dir(&cwd)
        .env("GAIA_CLONE_TEST", "yes")
        .env_remove("GAIA_CLONE_REMOVE");

    let cloned = clone_command(&command);

    assert_eq!(cloned.get_program(), OsStr::new("echo"));
    assert_eq!(
        cloned.get_args().collect::<Vec<_>>(),
        vec![OsStr::new("hello")]
    );
    assert_eq!(cloned.get_current_dir(), Some(cwd.as_path()));
    let envs = cloned.get_envs().collect::<Vec<_>>();
    assert!(envs.iter().any(|(key, value)| {
        *key == OsStr::new("GAIA_CLONE_TEST") && value.as_deref() == Some(OsStr::new("yes"))
    }));
    assert!(
        envs.iter()
            .any(|(key, value)| *key == OsStr::new("GAIA_CLONE_REMOVE") && value.is_none())
    );
}

#[test]
fn run_command_retains_bounded_output_tail() {
    let result = run_command_with_timeout(
        Command::new("bash").arg("-lc").arg(format!(
            "for i in $(seq 1 {}); do printf 'stdout-%04d\\n' \"$i\"; printf 'stderr-%04d\\n' \"$i\" >&2; done",
            MAX_RETAINED_STREAM_LINES + 10
        )),
        Duration::from_secs(10),
        "bounded-output-test",
        None,
        None,
    )
    .expect("command should complete");

    assert!(result.output.status.success());
    assert_eq!(result.stdout_lines.len(), MAX_RETAINED_STREAM_LINES);
    assert_eq!(result.stderr_lines.len(), MAX_RETAINED_STREAM_LINES);
    assert_eq!(
        result.stdout_lines.first().map(String::as_str),
        Some("stdout-0011")
    );
    assert_eq!(
        result.stderr_lines.first().map(String::as_str),
        Some("stderr-0011")
    );
    assert_eq!(
        result.stdout_lines.last().map(String::as_str),
        Some("stdout-1010")
    );
    assert_eq!(
        result.stderr_lines.last().map(String::as_str),
        Some("stderr-1010")
    );
    assert!(result.output.stdout.len() <= MAX_RETAINED_STREAM_BYTES);
    assert!(result.output.stderr.len() <= MAX_RETAINED_STREAM_BYTES);
}

#[test]
fn run_command_uses_configured_output_retention() {
    let result = run_command_with_timeout_and_retention(
        Command::new("bash").arg("-lc").arg(
            "printf 'stdout-1\\nstdout-2\\nstdout-3\\n'; printf 'stderr-1\\nstderr-2\\nstderr-3\\n' >&2",
        ),
        Duration::from_secs(10),
        "configured-output-retention-test",
        ProcessOutputRetention {
            stdout_bytes: 9,
            stderr_bytes: 9,
            stdout_lines: 2,
            stderr_lines: 1,
        },
        None,
        None,
    )
    .expect("command should complete");

    assert_eq!(result.stdout_lines, vec!["stdout-2", "stdout-3"]);
    assert_eq!(result.stderr_lines, vec!["stderr-3"]);
    assert!(result.output.stdout.len() <= 9);
    assert!(result.output.stderr.len() <= 9);
}

fn unique_dir(name: &str) -> PathBuf {
    let counter = TEST_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join("gaia-tests").join(format!(
        "gaia-process-{name}-{}-{counter}",
        std::process::id()
    ))
}
