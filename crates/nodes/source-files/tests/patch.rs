//! `gaia.source.patch`: patch series applied to a source tree by GNU
//! patch, strictly, in `series` order or by name.

use daedalus_cpu_author::FanIn;
use gaia_source_files::source_patch;
use gaia_step::{Ctx, Store};
use gaia_types::Tree;
use std::path::{Path, PathBuf};

const PATCH: &str = "/usr/bin/patch";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

/// A unified diff of `file` from `old` to `new` (whole-file hunks).
fn diff(file: &str, old: &str, new: &str) -> String {
    let lines = |t: &str| t.lines().count();
    let mut out = format!(
        "--- a/{file}\n+++ b/{file}\n@@ -1,{} +1,{} @@\n",
        lines(old),
        lines(new)
    );
    for line in old.lines() {
        out.push_str(&format!("-{line}\n"));
    }
    for line in new.lines() {
        out.push_str(&format!("+{line}\n"));
    }
    out
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    store: String,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let store = root.join("store").display().to_string();
        Self {
            _dir: dir,
            root,
            store,
        }
    }

    fn tree(&self, name: &str, files: &[(&str, &str)]) -> Tree {
        let dir = self.root.join(name);
        std::fs::create_dir_all(&dir).expect("mkdir");
        for (path, text) in files {
            write(&dir.join(path), text);
        }
        let store = Store::open(&self.store).expect("store");
        Tree {
            object: store.put_tree(&dir).expect("put").into(),
            entries: Vec::new(),
        }
    }

    fn source(&self) -> Tree {
        self.tree("src", &[("a.txt", "one\n"), ("sub/b.txt", "bee\n")])
    }

    /// The patched tree's directory, or the step's error.
    fn patch(&self, source: Tree, patches: &[(Tree, &str)]) -> Result<PathBuf, String> {
        let trees: Vec<Option<Tree>> = patches.iter().map(|(t, _)| Some(t.clone())).collect();
        let paths: Vec<&str> = patches.iter().map(|(_, p)| *p).collect();
        let tree = source_patch(
            &self.store,
            PATCH,
            source,
            FanIn::values(&trees),
            &serde_json::to_string(&paths).expect("json"),
            &Ctx::new(),
        )
        .map_err(|e| e.to_string())?;
        let store = Store::open(&self.store).expect("store");
        Ok(store.path(&tree.object.object_ref().expect("stored")))
    }
}

fn read(dir: &Path, path: &str) -> String {
    std::fs::read_to_string(dir.join(path)).expect(path)
}

#[test]
fn a_series_is_applied_in_its_order_and_a_directory_by_name() {
    if !Path::new(PATCH).exists() {
        println!("SKIPPED: no {PATCH}");
        return;
    }
    let f = Fixture::new();
    // The series file orders them against their names: two then one.
    let series = f.tree(
        "series",
        &[
            ("z-first.patch", &diff("a.txt", "one\n", "two\n")),
            ("a-second.patch", &diff("a.txt", "two\n", "three\n")),
            ("series", "# the order\nz-first.patch\na-second.patch -p1\n"),
        ],
    );
    let out = f
        .patch(f.source(), &[(series.clone(), "")])
        .expect("applied");
    assert_eq!(read(&out, "a.txt"), "three\n");
    assert_eq!(
        read(&out, "sub/b.txt"),
        "bee\n",
        "what no patch touches stays"
    );

    // No series: the *.patch files by name, the rest left alone; a second
    // tree, a file in it, after the first.
    let named = f.tree(
        "named",
        &[
            ("0001-x.patch", &diff("a.txt", "one\n", "two\n")),
            ("0002-y.patch", &diff("a.txt", "two\n", "three\n")),
            ("README", "not a patch"),
        ],
    );
    let more = f.tree(
        "more",
        &[("fix/b.patch", &diff("sub/b.txt", "bee\n", "wasp\n"))],
    );
    let out = f
        .patch(f.source(), &[(named, ""), (more, "fix/b.patch")])
        .expect("applied");
    assert_eq!(read(&out, "a.txt"), "three\n");
    assert_eq!(read(&out, "sub/b.txt"), "wasp\n");

    // Another patch's bytes, of the same name and length, are another step.
    for (dir, word) in [("other-a", "uno"), ("other-b", "una")] {
        let text = diff("a.txt", "one\n", &format!("{word}\n"));
        let other = f.tree(dir, &[("p.patch", &text)]);
        let out = f.patch(f.source(), &[(other, "p.patch")]).expect("applied");
        assert_eq!(read(&out, "a.txt"), format!("{word}\n"));
    }
}

#[test]
fn a_patch_that_does_not_apply_exactly_is_refused_by_name() {
    if !Path::new(PATCH).exists() {
        println!("SKIPPED: no {PATCH}");
        return;
    }
    let f = Fixture::new();
    let bad = f.tree(
        "bad",
        &[
            ("0001-ok.patch", &diff("a.txt", "one\n", "two\n")),
            ("0002-stale.patch", &diff("a.txt", "one\n", "three\n")),
        ],
    );
    let error = f.patch(f.source(), &[(bad, "")]).expect_err("stale");
    assert!(
        error.contains("patch 2 of 2, ./0002-stale.patch, does not apply"),
        "{error}"
    );
    // One line of context that differs is no match: patch's default fuzz
    // would apply this hunk; here it is refused.
    let numbers = f.tree("numbers", &[("n.txt", "1\n2\n3\n4\n5\n6\n7\n")]);
    let fuzzy = f.tree(
        "fuzzy",
        &[(
            "f.patch",
            "--- a/n.txt\n+++ b/n.txt\n@@ -1,7 +1,7 @@\n one\n 2\n 3\n-4\n+four\n 5\n 6\n 7\n",
        )],
    );
    let error = f.patch(numbers, &[(fuzzy, "f.patch")]).expect_err("fuzz");
    assert!(error.contains("f.patch, does not apply"), "{error}");

    for (files, path, why) in [
        (
            vec![("series", "gone.patch\n")],
            "",
            "series names gone.patch, which is not there",
        ),
        (
            vec![("series", "p.patch -p0\n"), ("p.patch", "x")],
            "",
            "p.patch is applied with -p0; every patch here is -p1",
        ),
        (vec![("README", "x")], "", "the patches given hold no patch"),
        (
            vec![("p.patch", "x")],
            "nope",
            "nope: no such patch or directory of patches",
        ),
        (
            vec![("p.patch", "x")],
            "../p.patch",
            "is not a path inside the tree",
        ),
    ] {
        let t = f.tree(&format!("case-{why:.8}"), &files);
        let error = f.patch(f.source(), &[(t, path)]).expect_err(why);
        assert!(error.contains(why), "{error}");
    }
}
