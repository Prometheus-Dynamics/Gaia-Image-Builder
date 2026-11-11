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
    let store = Store::open(dir.path()).expect("store");
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
}

#[test]
fn a_failed_body_leaves_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).expect("store");
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
            std::fs::read_dir(dir.path().join(sub)).expect("ls").count(),
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
    let store = Store::open(dir.path()).expect("store");
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
    let store = Store::open(dir.path()).expect("store");
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
    assert!(matches!(missing.kind, ErrorKind::Spawn(_)), "{missing}");
    let killed = run(Tool::new("/bin/sh").arg("-c").arg("kill -TERM $$"));
    assert_eq!(killed.kind, ErrorKind::Signal(15), "{killed}");
}

#[test]
fn a_failed_tool_error_holds_the_whole_stderr_tail() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).expect("store");
    let cx = Ctx::new();
    let error = Step::new(&store, &cx, "test.tool", KeyBuilder::new("x", 1).finish())
        .look_up_or_make(&[("out", Path::new("out"))], |work| {
            let script =
                "echo out; echo 'checking things' >&2; echo 'cannot find libfoo' >&2; exit 3";
            work.run(&Tool::new("/bin/sh").arg("-c").arg(script))?;
            Ok(())
        })
        .expect_err("fails");
    assert_eq!(error.kind, ErrorKind::Exit(3));
    assert_eq!(error.stderr_tail, ["checking things", "cannot find libfoo"]);
    assert_eq!(
        error.to_string(),
        "test.tool: exited 3: cannot find libfoo\nstderr, last lines:\n  checking things\n  cannot find libfoo"
    );
    assert_eq!(daedalus_cpu_author::FixedError::code(&error), 3);
}
