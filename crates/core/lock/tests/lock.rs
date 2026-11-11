use gaia_lock::{Entry, Lock, LockError, Want};

const COMMIT: &str = "3b9ee52ca0dbddcb64eb835e847f2059a8008ac5";

fn digest(byte: &str) -> String {
    format!("sha256:{}", byte.repeat(32))
}

fn sample() -> Lock {
    let mut lock = Lock::default();
    // Inserted out of order, with names that need quoting.
    lock.insert(
        "toolchain image",
        Entry::Image {
            reference: "docker.io/library/rust:1.99".into(),
            digest: digest("ab"),
        },
    );
    lock.insert(
        "kernel",
        Entry::Git {
            url: "https://user:secret@example.org/linux.git".into(),
            reference: "v7.3.3".into(),
            commit: COMMIT.into(),
        },
    );
    lock.insert(
        "blob \"ddr\"",
        Entry::Download {
            url: "https://example.org/firmware/ddr.bin".into(),
            digest: digest("01"),
        },
    );
    lock.insert(
        "overlay",
        Entry::Tree {
            path: "boards/x/overlay".into(),
            digest: digest("cd"),
        },
    );
    lock
}

fn wants() -> Vec<(&'static str, Want)> {
    vec![
        (
            "kernel",
            Want::Git {
                url: "https://user:secret@example.org/linux.git".into(),
                reference: "v7.3.3".into(),
            },
        ),
        (
            "blob \"ddr\"",
            Want::Download {
                url: "https://example.org/firmware/ddr.bin".into(),
            },
        ),
        (
            "toolchain image",
            Want::Image {
                reference: "docker.io/library/rust:1.99".into(),
            },
        ),
        (
            "overlay",
            Want::Tree {
                path: "boards/x/overlay".into(),
            },
        ),
    ]
}

fn check(lock: &Lock, wants: &[(&'static str, Want)]) -> Result<(), LockError> {
    lock.check(wants.iter().map(|(name, want)| (*name, want)))
}

const RECORDED: &str = r#"# Written by `gaia lock`. Do not edit.
version = 1

[source."blob \"ddr\""]
kind = "download"
url = "https://example.org/firmware/ddr.bin"
digest = "sha256:0101010101010101010101010101010101010101010101010101010101010101"

[source."kernel"]
kind = "git"
url = "https://example.org/linux.git"
ref = "v7.3.3"
commit = "3b9ee52ca0dbddcb64eb835e847f2059a8008ac5"

[source."overlay"]
kind = "tree"
path = "boards/x/overlay"
digest = "sha256:cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"

[source."toolchain image"]
kind = "image"
reference = "docker.io/library/rust:1.99"
digest = "sha256:abababababababababababababababababababababababababababababababab"
"#;

#[test]
fn write_and_read_round_trip_byte_for_byte() {
    let lock = sample();
    assert_eq!(lock.render(), RECORDED);
    let dir = tempfile::tempdir().expect("tempdir");
    let first = dir.path().join("product.lock");
    let second = dir.path().join("again.lock");
    lock.write(&first).expect("write");
    let read = Lock::read(&first).expect("read");
    assert_eq!(read, lock);
    read.write(&second).expect("write again");
    assert_eq!(
        std::fs::read(&first).expect("first"),
        std::fs::read(&second).expect("second")
    );
    assert_eq!(Lock::parse(RECORDED).expect("parse").render(), RECORDED);
    // No temporary file is left next to it.
    assert_eq!(std::fs::read_dir(dir.path()).expect("ls").count(), 2);
}

#[test]
fn a_lock_matching_the_descriptions_passes() {
    check(&sample(), &wants()).expect("matches");
    assert_eq!(sample().get("kernel").map(Entry::resolved), Some(COMMIT));
}

#[test]
fn a_missing_entry_fails_with_its_name() {
    let mut wants = wants();
    wants.push((
        "bootloader",
        Want::Git {
            url: "https://example.org/u-boot.git".into(),
            reference: "v2026.01".into(),
        },
    ));
    let error = check(&sample(), &wants).expect_err("missing");
    assert_eq!(
        error,
        LockError::Mismatch {
            missing: vec!["bootloader".into()],
            unused: vec![],
            changed: vec![]
        }
    );
    assert_eq!(
        error.to_string(),
        "the lock does not match the descriptions; run `gaia lock`\n  not in the lock: bootloader"
    );
}

#[test]
fn a_stale_entry_fails_with_its_name() {
    let wants: Vec<_> = wants()
        .into_iter()
        .filter(|(name, _)| *name != "overlay")
        .collect();
    let error = check(&sample(), &wants).expect_err("stale");
    assert_eq!(
        error,
        LockError::Mismatch {
            missing: vec![],
            unused: vec!["overlay".into()],
            changed: vec![]
        }
    );
    assert!(
        error
            .to_string()
            .ends_with("in the lock but used by nothing: overlay"),
        "{error}"
    );
}

#[test]
fn an_entry_locked_for_another_ref_fails_with_its_name() {
    let mut wants = wants();
    wants[0].1 = Want::Git {
        url: "https://example.org/linux.git".into(),
        reference: "v7.4".into(),
    };
    wants[2].1 = Want::Image {
        reference: "docker.io/library/rust:1.100".into(),
    };
    let error = check(&sample(), &wants).expect_err("changed");
    assert_eq!(
        error,
        LockError::Mismatch {
            missing: vec![],
            unused: vec![],
            changed: vec!["kernel".into(), "toolchain image".into()],
        }
    );
}

#[test]
fn every_problem_is_reported_at_once() {
    let wants = vec![
        (
            "kernel",
            Want::Tree {
                path: "kernel".into(),
            },
        ),
        (
            "new",
            Want::Download {
                url: "https://example.org/x".into(),
            },
        ),
    ];
    let error = check(&sample(), &wants).expect_err("several");
    assert_eq!(
        error,
        LockError::Mismatch {
            missing: vec!["new".into()],
            unused: vec![
                "blob \"ddr\"".into(),
                "overlay".into(),
                "toolchain image".into()
            ],
            changed: vec!["kernel".into()],
        }
    );
    println!("{error}");
}

#[test]
fn credentials_never_reach_the_lock() {
    assert!(!sample().render().contains("secret"));
    assert_eq!(
        gaia_lock::redact("https://a:b@host/x@y"),
        "https://host/x@y"
    );
    assert_eq!(gaia_lock::redact("git@host:repo.git"), "git@host:repo.git");
}

#[test]
fn a_lock_not_in_form_is_refused() {
    let bad = [
        RECORDED.replace("version = 1", "version = 2"),
        RECORDED.replace(COMMIT, "main"),
        RECORDED.replace(&digest("01"), "md5:1234"),
        RECORDED.replace("kind = \"tree\"", "kind = \"svn\""),
        RECORDED.replace(
            "path = \"boards/x/overlay\"",
            "path = \"boards/x/overlay\"\nextra = 1",
        ),
    ];
    for text in bad {
        let error = Lock::parse(&text).expect_err("refused");
        assert!(matches!(error, LockError::Parse(_)), "{error}");
    }
}
