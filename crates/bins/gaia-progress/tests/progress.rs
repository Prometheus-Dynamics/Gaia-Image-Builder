//! Progress read from run logs and the step record, printed plain and on a
//! terminal, and a failure with its stderr tail.

use gaia_progress::{Options, Printer, Reader, STDERR_LINES, failure};
use std::fs;
use std::path::Path;

const KEY: &str = "sha256:00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const PID: u32 = 4242;

fn log(logs: &Path, step: &str, start: u128, pid: u32, body: &str) -> std::path::PathBuf {
    let dir = logs.join(step);
    fs::create_dir_all(&dir).expect("dir");
    let path = dir.join(format!("{start}-0011223344556677-{pid}-0.log"));
    fs::write(
        &path,
        format!("# step {step}\n# key {KEY}\n# tool /bin/sh -c work\n# start {start}\n{body}"),
    )
    .expect("log");
    path
}

fn record(logs: &Path, start: u128, end: u128, pid: u32, made: &str, step: &str) {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs.join("steps.log"))
        .expect("steps.log");
    writeln!(file, "{start} {end} {pid} {made} {step} {KEY}").expect("write");
}

fn options(logs: &Path, terminal: bool) -> Options {
    Options {
        logs: logs.to_path_buf(),
        pid: PID,
        since_ns: 1_000,
        total: 3,
        terminal,
        width: 80,
    }
}

fn update(printer: &mut Printer) -> String {
    let mut out = Vec::new();
    printer.update(&mut out).expect("update");
    String::from_utf8(out).expect("utf-8")
}

#[test]
fn plain_lines_name_each_start_and_each_done_step_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let logs = dir.path();
    // Running, ended, another process's, and older than the build.
    log(logs, "test.run", 2_000, PID, "out| compiling\n");
    log(
        logs,
        "test.ended",
        1_500,
        PID,
        "out| x\n# end 1600 exit 0\n",
    );
    log(logs, "test.other", 2_000, PID + 1, "");
    log(logs, "test.old", 500, PID, "");
    record(logs, 1_500, 2_001_000_000, PID, "made", "test.ended");
    record(logs, 1_500, 1_600, PID + 1, "made", "test.other");

    let mut printer = Printer::new(options(logs, false));
    let first = update(&mut printer);
    assert_eq!(
        first,
        "start test.run  /bin/sh -c work\n[1/3] made      2000 ms  test.ended\n"
    );
    // Nothing new, nothing printed.
    assert_eq!(update(&mut printer), "");

    fs::write(
        logs.join("test.run")
            .join(format!("2000-0011223344556677-{PID}-0.log")),
        "# step test.run\n# end 3000 exit 0\n",
    )
    .expect("end");
    record(logs, 2_000, 3_000, PID, "hit", "test.run");
    assert_eq!(update(&mut printer), "[2/3] hit          0 ms  test.run\n");
}

#[test]
fn a_terminal_redraws_the_running_block_in_place() {
    let dir = tempfile::tempdir().expect("tempdir");
    let logs = dir.path();
    log(
        logs,
        "test.run",
        2_000,
        PID,
        "out| compiling\nerr| warning: slow\n",
    );

    let mut printer = Printer::new(options(logs, true));
    let first = update(&mut printer);
    let lines: Vec<&str> = first.lines().collect();
    assert_eq!(lines.len(), 2, "{first}");
    assert!(lines[0].starts_with("[0/3] ") && lines[0].ends_with(", 1 running"));
    assert!(lines[1].starts_with("  test.run  ") && lines[1].ends_with("  warning: slow"));
    // The next draw first moves up over the two lines and clears them.
    assert!(update(&mut printer).starts_with("\x1b[2A\r\x1b[J"));
    let mut out = Vec::new();
    printer.clear(&mut out).expect("clear");
    assert_eq!(out, b"\x1b[2A\r\x1b[J");
}

#[test]
fn a_failure_shows_the_last_stderr_lines_of_its_failed_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    let logs = dir.path();
    let mut body = String::new();
    for n in 0..30 {
        body.push_str(&format!("err| line {n}\nout| noise {n}\n"));
    }
    body.push_str("# end 9000 exit 3\n");
    let failed = log(logs, "test.tool", 2_000, PID, &body);
    // An earlier run that passed, and a later one from another process.
    log(
        logs,
        "test.tool",
        1_500,
        PID,
        "err| fine\n# end 1600 exit 0\n",
    );
    log(logs, "test.tool", 3_000, PID + 1, "# end 3100 exit 1\n");

    let reason = "test.tool: /bin/sh exited 3";
    let step = gaia_progress::reason_step(reason).expect("a step");
    let reader = Reader::new(logs.to_path_buf(), PID, 1_000);
    let none = std::collections::BTreeSet::new();
    let found = reader
        .failed_log(step, reason, &none)
        .expect("the failed log");
    assert_eq!(found, failed);

    let shown = failure("image.rootfs", reason, Some(&found));
    let lines: Vec<&str> = shown.lines().collect();
    assert_eq!(lines[0], "failed: image.rootfs");
    assert_eq!(lines[1], "  test.tool: /bin/sh exited 3");
    assert_eq!(lines[2], format!("  log: {}", failed.display()));
    assert_eq!(lines[3], format!("  stderr, last {STDERR_LINES} lines:"));
    assert_eq!(lines[4], "    line 10");
    assert_eq!(lines.last().copied(), Some("    line 29"));
    assert!(!shown.contains("noise"));

    // No log: the reason alone.
    assert_eq!(failure("x", "no tool", None), "failed: x\n  no tool\n");
    assert_eq!(gaia_progress::reason_step("a reason without a step"), None);
}

/// Three nodes of one step in one build: one fails and the two others are
/// cancelled, later. Each failure is shown its own log.
#[test]
fn each_of_several_failures_of_one_step_gets_its_own_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    let logs = dir.path();
    let failed = log(
        logs,
        "test.tool",
        2_000,
        PID,
        "err| no platform\n# end 2500 exit 2\n",
    );
    let first = log(logs, "test.tool", 2_100, PID, "# end 2600 cancelled\n");
    let second = log(logs, "test.tool", 2_200, PID, "# end 2600 cancelled\n");
    let reader = Reader::new(logs.to_path_buf(), PID, 1_000);
    let mut taken = std::collections::BTreeSet::new();
    let mut given = Vec::new();
    // In the order a build reports them: a cancelled one first.
    for reason in [
        "test.tool: cancelled",
        "test.tool: cancelled",
        "test.tool: exited 2",
    ] {
        let found = reader
            .failed_log("test.tool", reason, &taken)
            .expect("a log");
        taken.insert(found.clone());
        given.push(found);
    }
    assert_eq!(given, [second, first, failed]);
    // The one that failed, asked for first, is not a cancelled one's log.
    let none = std::collections::BTreeSet::new();
    assert_eq!(
        reader.failed_log("test.tool", "test.tool: /usr/bin/make exited 2", &none),
        Some(given[2].clone())
    );
}
