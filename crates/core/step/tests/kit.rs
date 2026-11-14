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

/// A line sink reads what the tool printed, so it has each line whole,
/// however long: a tool's JSON is one line. The kept tail is still cut.
#[test]
fn a_line_sink_has_a_long_line_whole() {
    use std::sync::{Arc, Mutex};
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let cx = Ctx::new();
    let key = KeyBuilder::new("test.long", 1).finish();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let mut tail = Vec::new();
    Step::new(&store, &cx, "test.long", key)
        .look_up_or_make(&[("out", Path::new("out"))], |work| {
            let tool = Tool::new("/bin/sh")
                .arg("-c")
                .arg("head -c 5000 /dev/zero | tr '\\0' x; echo")
                .on_line(move |_, line| sink.lock().expect("lock").push(line.len()));
            tail = work.run(&tool)?.stdout_tail;
            std::fs::write(work.dir().join("out"), "x").map_err(|e| StepError::body("t", e))
        })
        .expect("made");
    assert_eq!(*seen.lock().expect("lock"), [5000]);
    assert_eq!(tail[0].len(), 1024);
}

/// A body's own failure can be longer than the 120 bytes a fault record
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

/// One line that fits a fault record's reason needs no log; one byte more
/// and it has one.
#[test]
fn a_bodys_failure_gets_a_log_only_when_the_reason_cannot_hold_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("store");
    let cx = Ctx::new();
    let fail_with = |len: usize| {
        let key = KeyBuilder::new("t", 1)
            .config("len", len.to_string())
            .finish();
        // The reason is `t: <message>`.
        let message = "x".repeat(len - "t: ".len());
        Step::new(&store, &cx, "t", key)
            .look_up_or_make(&[("out", Path::new("out"))], |_| {
                Err(StepError::body("t", &message))
            })
            .expect_err("the body fails")
    };
    assert!(fail_with(120).log().is_none(), "120 bytes fit");
    assert!(fail_with(121).log().is_some(), "121 do not");
}

#[test]
fn two_makers_of_one_key_make_it_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("store");
    let ran = std::sync::atomic::AtomicUsize::new(0);
    let key = || {
        KeyBuilder::new("test.once", 1)
            .input("src", Digest::of_bytes(b"same"))
            .finish()
    };
    // Each maker would write its own bytes: an output that is not the same
    // twice, as a tool build's is not.
    let make = |text: &'static str| {
        let store = Store::open(&root).expect("store");
        let cx = Ctx::new();
        let made = Step::new(&store, &cx, "test.once", key())
            .look_up_or_make(&[("out", Path::new("out"))], |work| {
                ran.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(300));
                write_out(text)(work)
            })
            .expect("made");
        (made.made, made.outputs["out"])
    };
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| make("first"));
        let b = scope.spawn(|| make("second"));
        (a.join().expect("a"), b.join().expect("b"))
    });
    assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(a.0 != b.0, "one made it and one found it");
    assert_eq!(a.1, b.1, "both have the one recorded output");
}
