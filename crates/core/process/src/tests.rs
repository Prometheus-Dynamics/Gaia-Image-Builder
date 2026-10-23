use super::*;
use crate::retention::RetainedStream;
use gaia_testkit::TempDir;
use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

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

/// Every assembly transform and tool probe goes through this runner, so a
/// fast command must not pay a fixed delay (a poll interval, a flush, a sleep)
/// on top of its own run time.
#[cfg(unix)]
#[test]
fn run_command_returns_promptly_for_fast_command() {
    for _ in 0..3 {
        let started = Instant::now();
        let result = run_command_with_timeout(
            &mut Command::new("true"),
            Duration::from_secs(10),
            "fast-test",
            None,
            None,
        )
        .expect("true runs");
        let elapsed = started.elapsed();
        assert!(result.output.status.success());
        assert!(
            elapsed < Duration::from_millis(200),
            "a fast command took {elapsed:?} through the process runner"
        );
    }
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
    // A non-login shell: profile scripts only add startup time, which made the
    // 3 second bound flaky on a loaded machine.
    let result = run_command_with_timeout(
        Command::new("bash")
            .arg("-c")
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
    let scratch = TempDir::new("gaia-process-test");
    let dir = scratch.join("process-tree-timeout");
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

#[test]
fn run_command_bounds_stdout_without_newlines() {
    let result = run_command_with_timeout_and_retention(
        Command::new("python3")
            .arg("-c")
            .arg("import sys; sys.stdout.write('x' * 200000)"),
        Duration::from_secs(10),
        "long-stdout-line-retention-test",
        ProcessOutputRetention {
            stdout_bytes: 32,
            stderr_bytes: 32,
            stdout_lines: 4,
            stderr_lines: 4,
        },
        None,
        None,
    )
    .expect("command should complete");

    assert!(result.output.status.success());
    assert_eq!(result.output.stdout.len(), 32);
    assert!(result.output.stderr.is_empty());
    assert_eq!(result.stdout_lines.len(), 1);
    assert!(result.stdout_lines[0].len() <= MAX_RETAINED_LINE_BYTES);
}

#[test]
fn run_command_bounds_stderr_without_newlines() {
    let result = run_command_with_timeout_and_retention(
        Command::new("python3")
            .arg("-c")
            .arg("import sys; sys.stderr.write('e' * 200000)"),
        Duration::from_secs(10),
        "long-stderr-line-retention-test",
        ProcessOutputRetention {
            stdout_bytes: 32,
            stderr_bytes: 32,
            stdout_lines: 4,
            stderr_lines: 4,
        },
        None,
        None,
    )
    .expect("command should complete");

    assert!(result.output.status.success());
    assert!(result.output.stdout.is_empty());
    assert_eq!(result.output.stderr.len(), 32);
    assert_eq!(result.stderr_lines.len(), 1);
    assert!(result.stderr_lines[0].len() <= MAX_RETAINED_LINE_BYTES);
}

#[test]
fn run_command_bounds_fast_stdout_stream() {
    let result = run_command_with_timeout_and_retention(
        Command::new("python3")
            .arg("-c")
            .arg("import sys; sys.stdout.buffer.write((b'line\\n') * 50_000)"),
        Duration::from_secs(20),
        "fast-stdout-retention-test",
        ProcessOutputRetention {
            stdout_bytes: 128,
            stderr_bytes: 128,
            stdout_lines: 8,
            stderr_lines: 8,
        },
        None,
        None,
    )
    .expect("command should complete");

    assert!(result.output.status.success());
    assert!(result.output.stdout.len() <= 128);
    assert_eq!(result.stdout_lines.len(), 8);
    assert!(result.output.stderr.is_empty());
}

#[test]
fn run_command_bounds_fast_stderr_stream() {
    let result = run_command_with_timeout_and_retention(
        Command::new("python3")
            .arg("-c")
            .arg("import sys; sys.stderr.buffer.write((b'line\\n') * 50_000)"),
        Duration::from_secs(20),
        "fast-stderr-retention-test",
        ProcessOutputRetention {
            stdout_bytes: 128,
            stderr_bytes: 128,
            stdout_lines: 8,
            stderr_lines: 8,
        },
        None,
        None,
    )
    .expect("command should complete");

    assert!(result.output.status.success());
    assert!(result.output.stdout.is_empty());
    assert!(result.output.stderr.len() <= 128);
    assert_eq!(result.stderr_lines.len(), 8);
}

#[test]
fn run_command_retains_invalid_utf8_bytes() {
    let result = run_command_with_timeout_and_retention(
        Command::new("python3").arg("-c").arg(
            "import sys; sys.stdout.buffer.write(bytes([0xff, 0xfe, 10])); sys.stderr.buffer.write(bytes([0xfd, 0xfc, 10]))",
        ),
        Duration::from_secs(10),
        "invalid-utf8-retention-test",
        ProcessOutputRetention {
            stdout_bytes: 8,
            stderr_bytes: 8,
            stdout_lines: 2,
            stderr_lines: 2,
        },
        None,
        None,
    )
    .expect("command should complete");

    assert!(result.output.status.success());
    assert_eq!(result.output.stdout, vec![0xff, 0xfe, b'\n']);
    assert_eq!(result.output.stderr, vec![0xfd, 0xfc, b'\n']);
    assert_eq!(result.stdout_lines.len(), 1);
    assert_eq!(result.stderr_lines.len(), 1);
}

#[test]
fn run_command_retains_blank_line_bytes() {
    let result = run_command_with_timeout_and_retention(
        Command::new("python3")
            .arg("-c")
            .arg("import sys; sys.stdout.buffer.write(b'\\n\\n'); sys.stderr.buffer.write(b'\\n')"),
        Duration::from_secs(10),
        "blank-line-retention-test",
        ProcessOutputRetention {
            stdout_bytes: 8,
            stderr_bytes: 8,
            stdout_lines: 4,
            stderr_lines: 4,
        },
        None,
        None,
    )
    .expect("command should complete");

    assert!(result.output.status.success());
    assert_eq!(result.output.stdout, b"\n\n");
    assert_eq!(result.output.stderr, b"\n");
    assert_eq!(result.stdout_lines, vec!["", ""]);
    assert_eq!(result.stderr_lines, vec![""]);
}

#[cfg(unix)]
#[test]
fn run_command_retries_while_executable_is_busy() {
    let scratch = TempDir::new("gaia-process-test");
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let dir = scratch.join("etxtbsy");
    fs::create_dir_all(&dir).expect("dir");
    let script = dir.join("tool.sh");
    let mut writer = fs::File::create(&script).expect("script");
    writer
        .write_all(b"#!/bin/sh\necho ready\n")
        .expect("script body");
    writer.flush().expect("flush");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("mode");
    // Keep the script open for writing briefly, like a concurrently forked
    // child that inherited the descriptor.
    let releaser = thread::spawn(move || {
        thread::sleep(std::time::Duration::from_millis(40));
        drop(writer);
    });

    let result = run_command_with_timeout(
        &mut Command::new(&script),
        std::time::Duration::from_secs(10),
        "busy script",
        None,
        None,
    )
    .expect("spawn should retry until the script is released");
    releaser.join().expect("releaser");

    assert!(result.output.status.success());
    assert_eq!(result.stdout_lines, vec!["ready".to_string()]);
}

#[cfg(unix)]
#[test]
fn run_command_keeps_up_with_high_rate_line_output() {
    // 300k short lines: the drain loop must wait on the queue rather than
    // sleep between polls, or the bounded queue throttles the child.
    let started = std::time::Instant::now();
    let result = run_command_with_timeout(
        Command::new("sh")
            .arg("-c")
            .arg("yes line | head -n 300000; echo done"),
        std::time::Duration::from_secs(60),
        "chatty",
        None,
        None,
    )
    .expect("chatty command");

    assert!(result.output.status.success());
    assert_eq!(result.stdout_lines.last().map(String::as_str), Some("done"));
    assert_eq!(result.stdout_lines.len(), 1_000);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "took {:?}",
        started.elapsed()
    );
}

#[test]
fn retained_stream_keeps_newest_lines_and_bytes() {
    let mut retained = RetainedStream::with_limits(3, 5);
    for index in 0..10 {
        retained.push_line(format!("line {index}"));
    }
    retained.push_bytes(b"abc");
    retained.push_bytes(b"defg");
    assert_eq!(retained.take_bytes(), b"cdefg".to_vec());
    retained.push_bytes(b"0123456789");
    assert_eq!(retained.take_bytes(), b"56789".to_vec());
    assert_eq!(
        retained.take_lines(),
        vec!["line 7".to_string(), "line 8".into(), "line 9".into()]
    );

    let mut disabled = RetainedStream::with_limits(0, 0);
    disabled.push_line("ignored".into());
    disabled.push_bytes(b"ignored");
    assert!(disabled.take_lines().is_empty());
    assert!(disabled.take_bytes().is_empty());
}

#[test]
fn zero_retention_drains_a_chatty_command_and_keeps_nothing() {
    let result = run_command_with_timeout_and_retention(
        Command::new("sh")
            .arg("-c")
            .arg("seq 1 50000; seq 1 50000 >&2"),
        Duration::from_secs(20),
        "zero-retention-test",
        ProcessOutputRetention {
            stdout_bytes: 0,
            stderr_bytes: 0,
            stdout_lines: 0,
            stderr_lines: 0,
        },
        None,
        None,
    )
    .expect("command should complete");

    assert!(result.output.status.success());
    assert!(result.output.stdout.is_empty());
    assert!(result.output.stderr.is_empty());
    assert!(result.stdout_lines.is_empty());
    assert!(result.stderr_lines.is_empty());
}

#[test]
fn log_sink_sees_every_line_while_retention_keeps_the_newest() {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    let seen = Arc::new(AtomicUsize::new(0));
    let sink_seen = Arc::clone(&seen);
    let sink: ProcessLogSink = Arc::new(move |_line: ProcessLogLine| {
        sink_seen.fetch_add(1, Ordering::SeqCst);
    });
    let result = run_command_with_timeout_and_retention(
        Command::new("sh").arg("-c").arg("seq 1 30000"),
        Duration::from_secs(20),
        "sink-and-retention-test",
        ProcessOutputRetention {
            stdout_bytes: 0,
            stderr_bytes: 0,
            stdout_lines: 3,
            stderr_lines: 0,
        },
        Some(sink),
        None,
    )
    .expect("command should complete");

    assert_eq!(seen.load(Ordering::SeqCst), 30_000);
    // The newest lines, across the chunks the reader sent them in.
    assert_eq!(result.stdout_lines, vec!["29998", "29999", "30000"]);
}

#[test]
fn a_line_split_across_read_chunks_is_retained_whole() {
    let long = "x".repeat(20_000);
    let result = run_command_with_timeout_and_retention(
        Command::new("sh")
            .arg("-c")
            .arg(format!("printf 'a{long}\\nb\\n'")),
        Duration::from_secs(20),
        "split-line-test",
        ProcessOutputRetention {
            stdout_bytes: 1 << 20,
            stderr_bytes: 0,
            stdout_lines: 10,
            stderr_lines: 0,
        },
        None,
        None,
    )
    .expect("command should complete");

    assert_eq!(result.stdout_lines.len(), 2);
    assert_eq!(result.stdout_lines[0], format!("a{long}"));
    assert_eq!(result.stdout_lines[1], "b");
}

#[test]
fn a_build_that_echoes_a_secret_shows_it_masked_in_the_captured_log() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = TempDir::new("gaia-secret-echo-log");
    let token = scratch.join("token");
    fs::write(&token, "gaia-test-echo-value-4411\n").expect("write");
    fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).expect("mode");
    secrets::install(
        vec![secrets::SecretDef {
            name: "gaia_test_echo".into(),
            source: secrets::SecretSource::File(token),
            required: true,
        }],
        "",
    );
    let value = secrets::resolve("gaia_test_echo")
        .expect("resolve")
        .expect("set");

    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg("echo \"build used $GAIA_TEST_ECHO\"; echo \"warn $GAIA_TEST_ECHO\" >&2")
        .env("GAIA_TEST_ECHO", value.expose());
    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink_lines = captured.clone();
    let sink: ProcessLogSink = std::sync::Arc::new(move |line: ProcessLogLine| {
        sink_lines
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(line.line);
    });
    let result = run_command_with_timeout(
        &mut command,
        Duration::from_secs(30),
        "echo build",
        Some(sink),
        None,
    )
    .expect("the build runs");

    let lines = captured
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    assert!(
        lines.iter().any(|line| line == "build used ***"),
        "{lines:?}"
    );
    assert!(lines.iter().any(|line| line == "warn ***"), "{lines:?}");
    assert!(
        result
            .stdout_lines
            .iter()
            .chain(&result.stderr_lines)
            .all(|line| !line.contains(value.expose())),
        "{:?}",
        result.stdout_lines
    );
}
