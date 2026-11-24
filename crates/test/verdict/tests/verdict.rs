//! The key moves with each class of input, and only with what the case
//! reads; a verdict is reused only after a pass, and never where reuse is
//! off.

use gaia_verdict::{GATE_FILES, Place, Verdict, identity};
use std::path::Path;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

/// A repository with the gate's files, a fixture, a file no case reads,
/// and a stand-in test binary.
fn place(dir: &Path) -> Place {
    let root = dir.join("repo");
    for file in GATE_FILES {
        write(&root.join(file), "gate\n");
    }
    write(&root.join("Cargo.lock"), "version = 4\n");
    write(&root.join("fixtures/a/input.txt"), "a\n");
    write(&root.join("crates/other/src/lib.rs"), "\n");
    write(&dir.join("exe"), "binary one\n");
    Place {
        root,
        exe: dir.join("exe"),
        records: Some(dir.join("records")),
    }
}

fn key(place: &Place, tool: &str) -> String {
    Verdict::at("case", place, &[("go", tool.to_string())], &["fixtures/a"])
        .key()
        .to_string()
}

#[test]
fn the_key_moves_with_each_input_class_and_nothing_else() {
    let dir = tempfile::tempdir().expect("tempdir");
    let place = place(dir.path());
    let first = key(&place, "go1.25.4");
    assert_eq!(first.len(), 64);
    assert_eq!(
        key(&place, "go1.25.4"),
        first,
        "the same inputs, the same key"
    );

    // Code the case does not link, and files it does not read: no change.
    write(
        &place.root.join("crates/other/src/lib.rs"),
        "pub fn x() {}\n",
    );
    write(&place.root.join("Cargo.lock"), "version = 4\n# moved\n");
    write(&place.root.join("scripts/other.sh"), "echo\n");
    assert_eq!(key(&place, "go1.25.4"), first);

    let mut seen = vec![first.clone()];
    let mut moved = |class: &str, now: String| {
        assert!(!seen.contains(&now), "{class}: the key did not move");
        seen.push(now);
    };

    // The binary: the recipe, the pins, the node, store and sandbox code.
    write(&place.exe, "binary two\n");
    moved("binary", key(&place, "go1.25.4"));
    // The toolchain.
    moved("tool", key(&place, "go1.25.5"));
    // Each of the gate's files.
    for file in GATE_FILES {
        write(&place.root.join(file), &format!("gate, {file} changed\n"));
        moved(file, key(&place, "go1.25.5"));
    }
    // A file the case reads, inside a directory it reads.
    write(&place.root.join("fixtures/a/input.txt"), "b\n");
    moved("fixture", key(&place, "go1.25.5"));
    write(&place.root.join("fixtures/a/new.txt"), "new\n");
    moved("a new fixture", key(&place, "go1.25.5"));
    // A file outside the repository it boots or packs, by absolute path.
    let kernel = place.root.parent().expect("dir").join("kernel.img");
    write(&kernel, "kernel one\n");
    let with = |kernel_text: Option<&str>, pin: &str| {
        if let Some(text) = kernel_text {
            write(&kernel, text);
        }
        Verdict::at(
            "case",
            &place,
            &[("go", "go1.25.5".into()), ("image", pin.into())],
            &["fixtures/a", kernel.to_str().expect("utf8")],
        )
        .key()
        .to_string()
    };
    moved("an absolute file", with(None, "sha256:aa"));
    moved("its content", with(Some("kernel two\n"), "sha256:aa"));
    // A pin given as text: an image digest.
    moved("a pin", with(None, "sha256:bb"));
    // The case.
    moved(
        "case",
        Verdict::at(
            "other",
            &place,
            &[("go", "go1.25.5".into())],
            &["fixtures/a"],
        )
        .key()
        .to_string(),
    );
}

#[test]
fn a_verdict_is_reused_only_after_a_pass_and_never_with_reuse_off() {
    let dir = tempfile::tempdir().expect("tempdir");
    let place = place(dir.path());
    let verdict = Verdict::at("case", &place, &[], &[]);
    assert!(!verdict.reused(), "nothing passed yet");
    verdict.passed();
    assert!(Verdict::at("case", &place, &[], &[]).reused());

    // Another case, or the same case with a changed input: not reused.
    assert!(!Verdict::at("other", &place, &[], &[]).reused());
    write(&place.exe, "binary two\n");
    assert!(!Verdict::at("case", &place, &[], &[]).reused());

    // With reuse off (CI), a recorded pass is not reused.
    write(&place.exe, "binary one\n");
    let off = Place {
        records: None,
        ..place.clone()
    };
    let verdict = Verdict::at("case", &off, &[], &[]);
    assert!(!verdict.reused());
    verdict.passed();
    assert!(
        Verdict::at("case", &place, &[], &[]).reused(),
        "still there"
    );
}

#[test]
fn a_tools_identity_is_its_path_and_what_it_says() {
    let sh = identity(Path::new("/bin/sh"), &["-c", "echo one"]);
    assert!(sh.contains("one"), "{sh}");
    assert_ne!(sh, identity(Path::new("/bin/sh"), &["-c", "echo two"]));
    assert!(identity(Path::new("/no/such/tool"), &[]).contains("cannot run"));
    // A tool whose output varies (the time, a process id) is named the same
    // every time: by its content.
    let varies = || identity(Path::new("/bin/sh"), &["-c", "echo $$"]);
    assert_eq!(varies(), varies());
    assert!(varies().contains("its output varies"), "{}", varies());
}

#[test]
fn a_begun_case_records_its_pass_when_it_returns_and_not_when_it_panics() {
    let dir = tempfile::tempdir().expect("tempdir");
    let place = place(dir.path());
    let failing = Verdict::at("fails", &place, &[], &[]);
    let panicked = std::panic::catch_unwind(move || {
        let _pass = failing.begin().expect("not passed yet");
        panic!("the case fails");
    });
    assert!(panicked.is_err());
    assert!(
        !Verdict::at("fails", &place, &[], &[]).reused(),
        "a failure is no pass"
    );

    {
        let _pass = Verdict::at("passes", &place, &[], &[])
            .begin()
            .expect("not passed yet");
    }
    assert!(
        Verdict::at("passes", &place, &[], &[]).begin().is_none(),
        "returned: passed, and the next run reuses it"
    );
}
