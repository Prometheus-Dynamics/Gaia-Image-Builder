//! What a step's record keeps beyond its outputs: the CPU time of its tools,
//! the job count it was given, its details, and a failed body.

use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store, Tool};
use std::path::Path;

/// A tool that spins for a while: enough CPU to read back.
fn spin() -> Tool {
    Tool::new("/bin/sh")
        .arg("-c")
        .arg("i=0; while [ $i -lt 300000 ]; do i=$((i+1)); done")
        .env("PATH", "/usr/bin:/bin")
}

#[test]
fn a_tools_cpu_time_and_the_steps_report_are_recorded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let cx = Ctx::new();
    let key = KeyBuilder::new("test.report", 1).finish();
    Step::new(&store, &cx, "test.report", key)
        .look_up_or_make(&[("out", Path::new("out"))], |work| {
            work.jobs(4);
            work.detail("packages", serde_json::json!([{"name": "one"}]));
            work.run(&spin())?;
            std::fs::write(work.dir().join("out"), "x")
                .map_err(|e| StepError::body("test.report", e))
        })
        .expect("made");

    let records = gaia_step::step_records(&gaia_step::logs_dir(&store));
    assert_eq!(records.len(), 1);
    let report = &records[0].report;
    assert!(report.cpu_ns > 0, "a spinning tool used CPU: {report:?}");
    assert_eq!(report.jobs, Some(4));
    assert!(!report.failed);
    assert_eq!(report.details["packages"][0]["name"], "one");
}

#[test]
fn a_failed_body_is_recorded_with_what_it_measured() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let cx = Ctx::new();
    let key = KeyBuilder::new("test.failed", 1).finish();
    let error = Step::new(&store, &cx, "test.failed", key)
        .look_up_or_make(&[("out", Path::new("out"))], |work| {
            work.jobs(2);
            work.detail(
                "packages",
                serde_json::json!([{"name": "half", "open": "build"}]),
            );
            work.run(&spin())?;
            Err(StepError::body("test.failed", "broke"))
        })
        .expect_err("fails");
    assert_eq!(error.to_string(), "test.failed: broke");

    let records = gaia_step::step_records(&gaia_step::logs_dir(&store));
    assert_eq!(records.len(), 1, "a failed body has a record too");
    let record = &records[0];
    assert!(record.made, "the body ran");
    assert!(record.outputs.is_empty());
    assert!(record.report.failed);
    assert!(
        record.report.cpu_ns > 0,
        "the time before the failure is kept"
    );
    assert_eq!(record.report.jobs, Some(2));
    assert_eq!(record.report.details["packages"][0]["open"], "build");
}

#[test]
fn a_record_line_without_a_report_parses_with_an_empty_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let cx = Ctx::new();
    let key = KeyBuilder::new("test.old", 1).finish();
    Step::new(&store, &cx, "test.old", key)
        .look_up_or_make(&[("out", Path::new("out"))], |work| {
            std::fs::write(work.dir().join("out"), "x").map_err(|e| StepError::body("test.old", e))
        })
        .expect("made");
    // A line as written before reports existed: no tab, no JSON.
    let logs = gaia_step::logs_dir(&store);
    let path = logs.join("steps.log");
    let text = std::fs::read_to_string(&path).expect("steps.log");
    let old: String = text
        .lines()
        .map(|line| format!("{}\n", line.split_once('\t').map_or(line, |(head, _)| head)))
        .collect();
    std::fs::write(&path, old).expect("rewrite");

    let records = gaia_step::step_records(&logs);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].report, gaia_step::Report::default());
}

/// A record carries the step's shape and its inputs' digests, outside its
/// key: the same shape for the same step over other content, another shape
/// for a step with other config names.
#[test]
fn a_record_has_the_steps_shape_and_inputs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let cx = Ctx::new();
    let made = |key: gaia_step::StepKey| {
        Step::new(&store, &cx, "test.shape", key)
            .look_up_or_make(&[("out", Path::new("out"))], |work| {
                std::fs::write(work.dir().join("out"), "x")
                    .map_err(|e| StepError::body("test.shape", e))
            })
            .expect("made");
    };
    let input = gaia_step::Digest::of_bytes(b"an input");
    made(
        KeyBuilder::new("test.shape", 1)
            .config("fragment.a", "one")
            .input("source", input)
            .finish(),
    );
    made(
        KeyBuilder::new("test.shape", 1)
            .config("fragment.a", "two")
            .input("source", gaia_step::Digest::of_bytes(b"another"))
            .finish(),
    );
    made(
        KeyBuilder::new("test.shape", 1)
            .config("fragment.b", "one")
            .input("source", input)
            .finish(),
    );
    let records = gaia_step::step_records(&gaia_step::logs_dir(&store));
    let shapes: Vec<_> = records.iter().map(|r| r.report.shape.clone()).collect();
    assert!(shapes.iter().all(Option::is_some), "{shapes:?}");
    assert_eq!(shapes[0], shapes[1], "same names, other values");
    assert_ne!(shapes[0], shapes[2], "another config name");
    assert_eq!(records[0].report.inputs, vec![input.to_string()]);
    assert_ne!(records[0].key, records[1].key);
}

/// A tool that prints the same line without end is stopped once it has
/// printed it the given number of times in a row, and the error says why
/// and gives the line; the same line fewer times in a row, or lines that
/// differ, run to the end.
#[test]
fn a_tool_that_repeats_itself_without_end_is_stopped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let cx = Ctx::new();
    let key = KeyBuilder::new("test.repeat", 1).finish();
    let tool = |script: &str| {
        Tool::new("/bin/sh")
            .arg("-c")
            .arg(script.to_string())
            .env("PATH", "/usr/bin:/bin")
            .refuse_repeated(100, "asked again for ever")
    };
    let error = Step::new(&store, &cx, "test.repeat", key)
        .look_up_or_make(&[("out", Path::new("out"))], |work| {
            // Under the limit, and a long run of lines that differ.
            work.run(&tool(
                "yes 'Define X (X) [] (NEW)' | head -n 99; seq 1 5000",
            ))?;
            // `yes` never ends: only the refusal ends it.
            work.run(&tool("echo first; yes 'Define X (X) [] (NEW)'"))?;
            std::fs::write(work.dir().join("out"), "x").map_err(|e| StepError::body("t", e))
        })
        .expect_err("the endless run is refused")
        .to_string();
    assert!(
        error.contains(
            "asked again for ever (it printed 100 times in a row: Define X (X) [] (NEW))"
        ),
        "{error}"
    );
}

/// A tool run with `nice` runs, with what it starts, at that niceness; one
/// without at the step's own.
#[test]
fn a_niced_tool_and_its_children_run_at_that_niceness() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let cx = Ctx::new();
    let key = KeyBuilder::new("test.nice", 1).finish();
    // Field 19 of /proc/<pid>/stat is the niceness: the shell's own, then
    // a child's.
    let script = "cut -d' ' -f19 /proc/$$/stat; sh -c 'cut -d\" \" -f19 /proc/$$/stat'";
    let (mut niced, mut plain) = (Vec::new(), Vec::new());
    Step::new(&store, &cx, "test.nice", key)
        .look_up_or_make(&[("out", Path::new("out"))], |work| {
            let tool = || {
                Tool::new("/bin/sh")
                    .arg("-c")
                    .arg(script)
                    .env("PATH", "/usr/bin:/bin")
            };
            niced = work.run(&tool().nice(19))?.stdout_tail;
            plain = work.run(&tool())?.stdout_tail;
            std::fs::write(work.dir().join("out"), "x").map_err(|e| StepError::body("t", e))
        })
        .expect("made");
    let own = rustix::process::getpriority_process(None).expect("own niceness");
    assert_eq!(niced, ["19", "19"]);
    assert_eq!(plain, [own.to_string(), own.to_string()]);
}
