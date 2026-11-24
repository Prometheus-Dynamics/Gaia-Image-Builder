//! `scripts/acache/acache`, the artifact cache shared with the other
//! repositories: its contract, on a cache of its own.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn tool() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scripts/acache/acache")
}

fn acache(root: &Path, args: &[&str]) -> Output {
    Command::new("python3")
        .arg(tool())
        .args(args)
        .env("ACACHE_ROOT", root)
        .output()
        .expect("python3 runs")
}

fn text(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn the_cache_keys_by_inputs_builds_once_and_drops_the_least_used() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("cache");
    let path = |name: &str| dir.path().join(name).display().to_string();
    std::fs::write(path("m"), "src=abc\narch=x86_64\n").expect("write");
    std::fs::write(path("m2"), "# a note\narch=x86_64\nsrc=abc\n").expect("write");
    std::fs::write(path("other"), "src=abd\narch=x86_64\n").expect("write");

    // The key is the inputs', in any order, comments aside.
    let key = text(&acache(&root, &["key", "kernel", &path("m")]));
    assert_eq!(key.len(), 64, "{key}");
    assert_eq!(text(&acache(&root, &["key", "kernel", &path("m2")])), key);
    assert_ne!(
        text(&acache(&root, &["key", "kernel", &path("other")])),
        key
    );

    // A miss exits 1 and writes nothing.
    let miss = acache(&root, &["get", "kernel", &path("m"), &path("got")]);
    assert_eq!(miss.status.code(), Some(1));
    assert!(!dir.path().join("got").exists());

    // Four callers at once: one build, four copies of it.
    let count = path("count");
    let runs: Vec<_> = (0..4)
        .map(|i| {
            let dest = path(&format!("d{i}"));
            Command::new("python3")
                .arg(tool())
                .args([
                    "run",
                    "kernel",
                    &path("m"),
                    &dest,
                    "--producer",
                    "test",
                    "--",
                ])
                .args([
                    "sh",
                    "-c",
                    &format!(
                        "sleep 1; echo built >> {count}; mkdir {dest}; echo k > {dest}/bzImage"
                    ),
                ])
                .env("ACACHE_ROOT", &root)
                .spawn()
                .expect("spawns")
        })
        .collect();
    for mut run in runs {
        assert!(run.wait().expect("waits").success());
    }
    assert_eq!(std::fs::read_to_string(&count).expect("count"), "built\n");
    for i in 0..4 {
        assert_eq!(
            std::fs::read_to_string(dir.path().join(format!("d{i}/bzImage"))).expect("copy"),
            "k\n"
        );
    }

    // The entry re-hashes to its record, and a hit by the reordered inputs
    // is the same entry.
    assert_eq!(text(&acache(&root, &["verify", "kernel", &key])), "ok");
    let hit = text(&acache(&root, &["path", "kernel", &path("m2")]));
    assert!(hit.ends_with(&format!("kernel/{key}/out")), "{hit}");

    // A put of a key already there keeps the first.
    std::fs::write(path("new"), "changed\n").expect("write");
    assert!(
        acache(&root, &["put", "kernel", &path("m"), &path("new")])
            .status
            .success()
    );
    assert!(Path::new(&hit).join("bzImage").is_file(), "the first stays");

    // Over the cap (9 bytes against 2 + 8), the least recently used goes.
    assert!(
        acache(&root, &["put", "busybox", &path("other"), &path("new")])
            .status
            .success()
    );
    std::thread::sleep(std::time::Duration::from_millis(20));
    let _ = acache(&root, &["path", "busybox", &path("other")]);
    let gc = acache(&root, &["gc", "--max-gib", "0.0000000084"]);
    assert!(gc.status.success(), "{gc:?}");
    let left = text(&acache(&root, &["ls"]));
    assert!(
        left.starts_with("busybox\t") && left.lines().count() == 1,
        "{left}"
    );
}
