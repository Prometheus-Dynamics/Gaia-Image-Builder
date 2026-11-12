//! The report node on small stored trees: contents, owners, sources with
//! secrets masked, a delta against a previous report, and determinism.

use gaia_lock::{Entry, Lock};
use gaia_report::{Report, mask_url, render_text, report_layers};
use gaia_step::Ctx;
use gaia_store::Store;
use gaia_types::Layer;
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::path::{Path, PathBuf};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    store: String,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let store = root.join("store").display().to_string();
    Fixture {
        _dir: dir,
        root,
        store,
    }
}

fn write_files(dir: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, text).expect("write");
    }
}

/// A layer for `owner` whose tree holds `files`.
fn layer(f: &Fixture, owner: &str, files: &[(&str, &str)]) -> Layer {
    let dir = f.root.join("src").join(owner);
    let _ = std::fs::remove_dir_all(&dir);
    write_files(&dir, files);
    let store = Store::open(&f.store).expect("store");
    Layer {
        owner: owner.into(),
        tree: Tree {
            object: store.put_tree(&dir).expect("put").into(),
            entries: Vec::new(),
        },
        replaces: Vec::new(),
    }
}

/// An image archive with `text` as its content.
fn image(f: &Fixture, name: &str, text: &str) -> Artifact {
    let path = f.root.join(name);
    std::fs::write(&path, text).expect("write");
    let store = Store::open(&f.store).expect("store");
    Artifact {
        kind: ArtifactKind::Archive,
        object: store.put_file(&path).expect("put").into(),
    }
}

fn read(f: &Fixture, artifact: &Artifact) -> Report {
    let store = Store::open(&f.store).expect("store");
    let object = artifact.object.object_ref().expect("ref");
    let text = std::fs::read(store.path(&object)).expect("read");
    serde_json::from_slice(&text).expect("json")
}

fn lock() -> Lock {
    let mut lock = Lock::default();
    lock.insert(
        "hello-a.src",
        Entry::Git {
            url: "https://user:pw@example.org/hello.git".into(),
            reference: "master".into(),
            commit: "3b9ee52ca0dbddcb64eb835e847f2059a8008ac5".into(),
        },
    );
    lock.insert(
        "hello-b.src",
        Entry::Download {
            url: "https://example.org/hello-b.tgz?access_token=s3cr3t".into(),
            digest: format!("sha256:{}", "0".repeat(64)),
        },
    );
    lock
}

#[test]
fn every_file_has_its_owner_and_sources_are_masked() {
    let f = fixture();
    let a = layer(
        &f,
        "hello-a",
        &[("bin/hello", "0123456789"), ("lib/libx.so", "abc")],
    );
    let b = layer(&f, "hello-b", &[("bin/other", "xy")]);
    let img = image(&f, "a.tar", "tar bytes");
    let out = report_layers(
        &f.store,
        "a",
        &[a, b],
        &img,
        &lock().render(),
        "",
        &Ctx::new(),
    )
    .expect("reports");
    let report = read(&f, &out);

    assert_eq!(report.format, "gaia.report");
    assert_eq!(report.image, "a");
    assert_eq!(report.image_bytes, 9);
    let paths: Vec<(&str, &str)> = report
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.owners[0].as_str()))
        .collect();
    assert_eq!(
        paths,
        [
            ("bin/hello", "hello-a"),
            ("bin/other", "hello-b"),
            ("lib/libx.so", "hello-a"),
        ]
    );
    assert_eq!(report.total_bytes, 10 + 2 + 3);
    let sizes: Vec<(&str, u64)> = report
        .components
        .iter()
        .map(|c| (c.owner.as_str(), c.bytes))
        .collect();
    assert_eq!(sizes, [("hello-a", 13), ("hello-b", 2)]);

    // Sources are the lock's entries of each owner, masked.
    let names: Vec<String> = report
        .sources
        .iter()
        .map(|s| format!("{}.{}", s.owner, s.source))
        .collect();
    assert_eq!(names, ["hello-a.src", "hello-b.src"]);
    let text = std::fs::read_to_string(
        Store::open(&f.store)
            .expect("store")
            .path(&out.object.object_ref().expect("ref")),
    )
    .expect("read");
    assert!(!text.contains("s3cr3t"), "secret printed: {text}");
    assert!(!text.contains("user:pw"), "userinfo printed: {text}");
    assert!(text.contains("https://example.org/hello.git"));
    assert!(render_text(&report).contains("hello-a.src"));
    assert_eq!(
        mask_url("https://example.org/x?access_token=s3cr3t"),
        "https://example.org/x?access_token=********"
    );
    assert!(report.previous.is_none() && report.delta.is_none());
}

#[test]
fn a_changed_run_reports_its_delta_against_the_previous_one() {
    let f = fixture();
    let img = image(&f, "a.tar", "tar bytes");
    let before = report_layers(
        &f.store,
        "a",
        &[
            layer(
                &f,
                "hello-a",
                &[("bin/hello", "0123456789"), ("etc/old", "x")],
            ),
            layer(&f, "hello-b", &[("bin/other", "xy")]),
        ],
        &img,
        &lock().render(),
        "",
        &Ctx::new(),
    )
    .expect("first");
    let first_ref = before.object.object_ref().expect("ref").to_string();

    // A changed setting: hello-a's binary changes size, its etc/old is gone,
    // a new file appears, and the image is bigger.
    let img2 = image(&f, "a2.tar", "tar bytes, more");
    let after = report_layers(
        &f.store,
        "a",
        &[
            layer(
                &f,
                "hello-a",
                &[("bin/hello", "0123456789abc"), ("etc/new", "y")],
            ),
            layer(&f, "hello-b", &[("bin/other", "xy")]),
        ],
        &img2,
        &lock().render(),
        &first_ref,
        &Ctx::new(),
    )
    .expect("second");
    let report = read(&f, &after);
    assert_eq!(report.previous.as_deref(), Some(first_ref.as_str()));
    let delta = report.delta.clone().expect("delta");
    assert_eq!(delta.added, ["etc/new"]);
    assert_eq!(delta.removed, ["etc/old"]);
    assert_eq!(delta.changed.len(), 1);
    assert_eq!(delta.changed[0].path, "bin/hello");
    assert_eq!(delta.changed[0].before_bytes, 10);
    assert_eq!(delta.changed[0].after_bytes, 13);
    assert_eq!(delta.image_change, 6);
    assert_eq!(delta.total_change, 3);
    let owners: Vec<(&str, i64)> = delta
        .components
        .iter()
        .map(|c| (c.owner.as_str(), c.after as i64 - c.before as i64))
        .collect();
    assert_eq!(owners, [("hello-a", 3)]);
    assert!(delta.sources.is_empty(), "same lock, same sources");
    let shown = render_text(&report);
    assert!(shown.contains("removed  etc/old"), "{shown}");
}

#[test]
fn the_same_inputs_give_the_same_report() {
    let f = fixture();
    let img = image(&f, "a.tar", "tar bytes");
    let inputs = || {
        report_layers(
            &f.store,
            "a",
            &[layer(&f, "hello-a", &[("bin/hello", "0123")])],
            &img,
            "",
            "",
            &Ctx::new(),
        )
        .expect("reports")
    };
    let first = inputs();
    let second = inputs();
    assert_eq!(first, second);
}

#[test]
fn a_previous_that_is_not_a_report_is_refused() {
    let f = fixture();
    let img = image(&f, "a.tar", "tar bytes");
    let not_a_report = image(&f, "other.json", "{\"format\": \"something\"}");
    let text = not_a_report.object.object_ref().expect("ref").to_string();
    let err = report_layers(
        &f.store,
        "a",
        &[layer(&f, "hello-a", &[("bin/hello", "0123")])],
        &img,
        "",
        &text,
        &Ctx::new(),
    )
    .expect_err("refused");
    assert!(err.to_string().contains("the previous report"), "{err}");
}
