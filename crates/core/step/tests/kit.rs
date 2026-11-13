//! Look up or make, the key, and a tool's environment, without a host.

use gaia_step::{Check, Ctx, Digest, ErrorKind, KeyBuilder, Step, StepError, Store, Tool};
use std::cell::Cell;
use std::path::Path;

fn write_out(text: &'static str) -> impl FnOnce(&mut gaia_step::Work<'_>) -> Result<(), StepError> {
    move |work| std::fs::write(work.dir().join("out"), text).map_err(|e| StepError::body("test", e))
}

#[test]
fn a_second_run_with_the_same_key_does_not_run_the_body() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let cx = Ctx::new();
    let ran = Cell::new(0);
    let key = |input: &[u8]| {
        KeyBuilder::new("test.make", 1)
            .input("src", Digest::of_bytes(input))
            .finish()
    };
    let outputs = [("out", Path::new("out"))];
    let make = |input: &[u8]| {
        Step::new(&store, &cx, "test.make", key(input))
            .look_up_or_make(&outputs, |work| {
                ran.set(ran.get() + 1);
                write_out("made")(work)
            })
            .expect("made")
    };
    let first = make(b"v1");
    assert!(first.made);
    let second = make(b"v1");
    assert!(!second.made);
    assert_eq!(second.outputs, first.outputs);
    assert_eq!(ran.get(), 1);

    // A changed input makes again; a removed output makes again.
    assert!(make(b"v2").made);
    std::fs::remove_file(store.path(&first.outputs["out"])).expect("rm");
    assert!(make(b"v1").made);
    assert_eq!(ran.get(), 3);

    // A changed output is found by the digest check, not by a presence check.
    std::fs::write(store.path(&first.outputs["out"]), "tampered").expect("write");
    let present = Step::new(&store, &cx, "test.make", key(b"v1"))
        .check(Check::Present)
        .look_up_or_make(&outputs, write_out("made"))
        .expect("present");
    assert!(!present.made);
    assert!(make(b"v1").made);

    // Every look-up-or-make is in the step record, in order.
    let records = gaia_step::step_records(&gaia_step::logs_dir(&store));

    // Read from an offset, the same records come back, a few at a time.
    let logs = gaia_step::logs_dir(&store);
    let (head, offset) = gaia_step::step_records_from(&logs, 0);
    assert_eq!(head, records);
    assert_eq!(
        gaia_step::step_records_from(&logs, offset),
        (Vec::new(), offset)
    );
    make(b"v2");
    let (tail, next) = gaia_step::step_records_from(&logs, offset);
    assert_eq!(tail.len(), 1);
    assert!(next > offset);
    let seen: Vec<_> = records
        .iter()
        .map(|record| (record.made, record.key == key(b"v1")))
        .collect();
    assert_eq!(
        seen,
        [
            (true, true),
            (false, true),
            (true, false),
            (true, true),
            (false, true),
            (true, true)
        ]
    );
    assert!(records.iter().all(|record| record.step == "test.make"
        && record.pid == std::process::id()
        && record.start_ns <= record.end_ns));
    assert_eq!(records[1].outputs, first.outputs);
}

#[test]
fn a_failed_body_leaves_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let cx = Ctx::new();
    let key = KeyBuilder::new("test.fail", 1).finish();
    let error = Step::new(&store, &cx, "test.fail", key)
        .look_up_or_make(&[("out", Path::new("out"))], |work| {
            std::fs::write(work.dir().join("out"), "half").expect("write");
            Err(StepError::body("test.fail", "broke"))
        })
        .expect_err("fails");
    assert_eq!(error.to_string(), "test.fail: broke");
    assert!(store.entry(&key).expect("entry").is_none());
    for sub in ["objects/file", "scratch"] {
        assert_eq!(
            std::fs::read_dir(store.root().join(sub))
                .expect("ls")
                .count(),
            0
        );
    }
}

#[test]
fn keys_cover_every_part_and_ignore_order() {
    let base = || KeyBuilder::new("k", 1).config("a", "1").config("b", "2");
    let digest = Digest::of_bytes(b"x");
    let key = base().input("i", digest).finish();
    assert_eq!(
        KeyBuilder::new("k", 1)
            .input("i", digest)
            .config("b", "2")
            .config("a", "1")
            .finish(),
        key
    );
    for other in [
        KeyBuilder::new("k2", 1)
            .config("a", "1")
            .config("b", "2")
            .input("i", digest),
        KeyBuilder::new("k", 2)
            .config("a", "1")
            .config("b", "2")
            .input("i", digest),
        base().input("i", Digest::of_bytes(b"y")),
        base().input("j", digest),
        KeyBuilder::new("k", 1).config("a", "12").input("i", digest),
        KeyBuilder::new("k", 1)
            .config("a", "1")
            .config("b", "3")
            .input("i", digest),
    ] {
        assert_ne!(other.finish(), key);
    }
    println!("recorded key: {}", key.0);
}

#[test]
fn a_tool_sees_only_the_environment_it_is_given() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let cx = Ctx::new();
    let key = KeyBuilder::new("test.env", 1).finish();
    let mut seen = Vec::new();
    Step::new(&store, &cx, "test.env", key)
        .look_up_or_make(&[("out", Path::new("out"))], |work| {
            let output = work.run(&Tool::new("/usr/bin/env").env("ONLY", "this"))?;
            seen = output.stdout_tail;
            std::fs::write(work.dir().join("out"), "x").map_err(|e| StepError::body("t", e))
        })
        .expect("made");
    assert_eq!(seen, ["ONLY=this"]);
}

#[test]
fn a_missing_program_and_a_signal_are_faults() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let cx = Ctx::new();
    let run = |tool: Tool| {
        Step::new(&store, &cx, "test.tool", KeyBuilder::new("x", 1).finish())
            .look_up_or_make(&[("out", Path::new("out"))], |work| {
                work.run(&tool)?;
                Ok(())
            })
            .expect_err("fails")
    };
    let missing = run(Tool::new("/nonexistent/tool"));
    assert!(matches!(missing.kind(), ErrorKind::Spawn(_)), "{missing}");
    let killed = run(Tool::new("/bin/sh").arg("-c").arg("kill -TERM $$"));
    assert_eq!(killed.kind(), &ErrorKind::Signal(15), "{killed}");
}

#[test]
fn a_failed_tool_keeps_its_whole_output_in_a_run_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let cx = Ctx::new();
    let error = Step::new(&store, &cx, "test.tool", KeyBuilder::new("x", 1).finish())
        .look_up_or_make(&[("out", Path::new("out"))], |work| {
            let script =
                "echo out; echo 'checking things' >&2; echo 'cannot find libfoo' >&2; exit 3";
            work.run(&Tool::new("/bin/sh").arg("-c").arg(script))?;
            Ok(())
        })
        .expect_err("fails");
    assert_eq!(error.kind(), &ErrorKind::Exit(3));
    assert_eq!(
        error.stderr_tail(),
        ["checking things", "cannot find libfoo"]
    );
    assert_eq!(
        daedalus_cpu_author::FixedError::reason(&error),
        "test.tool: exited 3"
    );
    let log = error.log().expect("a run log");
    assert!(
        log.starts_with(dir.path().join("logs").join("test.tool")),
        "{log:?}"
    );
    assert_eq!(
        error.to_string(),
        format!(
            "test.tool: exited 3\nlog: {}\nstderr, last lines:\n  checking things\n  cannot find libfoo",
            log.display()
        )
    );
    // The log has everything, each stream in order, and outlives the
    // failed step's scratch. stdout and stderr are read on two threads, so
    // how their lines interleave is not fixed.
    let text = std::fs::read_to_string(log).expect("log");
    let stream = |prefix: &str| -> Vec<String> {
        text.lines()
            .filter(|line| line.starts_with(prefix))
            .map(str::to_string)
            .collect()
    };
    assert_eq!(stream("out| "), ["out| out"]);
    assert_eq!(
        stream("err| "),
        ["err| checking things", "err| cannot find libfoo"]
    );
    assert_eq!(
        text.lines().filter(|line| !line.starts_with('#')).count(),
        3,
        "{text}"
    );
    assert!(
        text.starts_with("# step test.tool\n# key sha256:"),
        "{text}"
    );
    assert!(text.contains("\n# tool /bin/sh -c echo out;"), "{text}");
    assert!(text.trim_end().ends_with(" exit 3"), "{text}");
    assert_eq!(daedalus_cpu_author::FixedError::code(&error), 3);
}

/// A body's own failure can be longer than the 64 bytes a fault record
/// keeps of its reason, so the whole message goes to a run log the failure
/// names: a check that lists what it found does not lose the list.
#[test]
fn a_bodys_own_failure_is_whole_in_a_run_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let cx = Ctx::new();
    let key = KeyBuilder::new("test.check", 1).finish();
    let message = "the config did not take 2 lines as written:\n  a.fragment: `CONFIG_A=y` became `CONFIG_A=m`\n  b.fragment: `CONFIG_B=y` was dropped";
    let error = Step::new(&store, &cx, "test.check", key)
        .look_up_or_make(&[("out", Path::new("out"))], |_| {
            Err(StepError::body("test.check", message))
        })
        .expect_err("the body fails");
    assert!(matches!(error.kind(), ErrorKind::Body(_)));
    let log = error.log().expect("the failure names a log");
    let text = std::fs::read_to_string(log).expect("read the log");
    println!("{text}");
    for line in message.lines() {
        assert!(
            text.contains(&format!("err| {line}")),
            "the log has {line:?}"
        );
    }
    assert!(
        text.lines().last().is_some_and(|l| l.ends_with(" failed")),
        "the log ends as failed, so it is the one a failed build shows"
    );
}
