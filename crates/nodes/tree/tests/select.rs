//! `gaia.tree.select`: declared files of one tree, added to another at the
//! paths named, and the step keyed on those files alone.

use gaia_step::{Ctx, Store, logs_dir, step_records};
use gaia_tree::{tree_select, tree_text};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::path::Path;

/// A directory of files as an artifact, as a kernel build gives its
/// device trees.
fn outputs(root: &Path, store: &str, name: &str, files: &[(&str, &str)]) -> Artifact {
    Artifact {
        kind: ArtifactKind::parse("tree"),
        object: stored(root, store, name, files).object,
    }
}

fn stored(root: &Path, store: &str, name: &str, files: &[(&str, &str)]) -> Tree {
    let dir = root.join(name);
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }
    let store = Store::open(store).expect("store");
    Tree {
        object: store.put_tree(&dir).expect("put").into(),
        entries: Vec::new(),
    }
}

/// Every file of a stored tree, with its text.
fn files(store: &str, tree: &Tree) -> Vec<(String, String)> {
    let opened = Store::open(store).expect("store");
    let root = tree.object.path_in(&opened).expect("in the store");
    let mut out = Vec::new();
    let mut dirs = vec![root.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("read dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                let name = path
                    .strip_prefix(&root)
                    .expect("inside")
                    .display()
                    .to_string();
                out.push((name, std::fs::read_to_string(&path).expect("read")));
            }
        }
    }
    out.sort();
    out
}

const BUILT: [(&str, &str); 4] = [
    ("board.dtb", "dtb"),
    ("other.dtb", "other"),
    ("gadget.dtbo", "overlay"),
    ("map.dtb", "map"),
];

#[test]
fn the_named_files_go_where_they_are_named() {
    let root = tempfile::tempdir().expect("tempdir");
    let store = root.path().join("store").display().to_string();
    let built = outputs(root.path(), &store, "built", &BUILT);
    let base = tree_text(&store, None, "config.txt", "a=1\n", &Ctx::new()).expect("text");
    let made = tree_select(
        &store,
        Some(&base),
        &built,
        "board.dtb\n  gadget.dtbo=overlays/gadget.dtbo /map.dtb=overlays/map.dtb",
        &Ctx::new(),
    )
    .expect("select");
    assert_eq!(
        files(&store, &made),
        [
            ("board.dtb", "dtb"),
            ("config.txt", "a=1\n"),
            ("overlays/gadget.dtbo", "overlay"),
            ("overlays/map.dtb", "map"),
        ]
        .map(|(a, b)| (a.to_string(), b.to_string()))
    );
    let paths: Vec<&str> = made.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "board.dtb",
            "config.txt",
            "overlays/gadget.dtbo",
            "overlays/map.dtb"
        ]
    );
    assert!(made.entries.iter().all(|e| e.mode == 0o644 && e.uid == 0));

    // Alone, with no tree to add to.
    let alone = tree_select(&store, None, &built, "other.dtb", &Ctx::new()).expect("select");
    assert_eq!(
        files(&store, &alone),
        [("other.dtb".to_string(), "other".to_string())]
    );

    // A file not selected changes: the same step, not made again.
    let mut changed = BUILT;
    changed[1] = ("other.dtb", "changed");
    let built2 = outputs(root.path(), &store, "built2", &changed);
    let again = tree_select(
        &store,
        Some(&base),
        &built2,
        "board.dtb gadget.dtbo=overlays/gadget.dtbo map.dtb=overlays/map.dtb",
        &Ctx::new(),
    )
    .expect("select");
    assert_eq!(again, made);
    // A selected one changes: made again.
    let mut changed = BUILT;
    changed[0] = ("board.dtb", "dtb 2");
    let built3 = outputs(root.path(), &store, "built3", &changed);
    let other = tree_select(&store, None, &built3, "board.dtb", &Ctx::new()).expect("select");
    assert_eq!(
        files(&store, &other),
        [("board.dtb".to_string(), "dtb 2".to_string())]
    );
    let opened = Store::open(&store).expect("store");
    let made_count = step_records(&logs_dir(&opened))
        .into_iter()
        .filter(|r| r.step == "gaia.tree.select" && r.made)
        .count();
    assert_eq!(made_count, 3, "one per distinct selection");
}

#[test]
fn nothing_is_guessed_or_replaced() {
    let root = tempfile::tempdir().expect("tempdir");
    let store = root.path().join("store").display().to_string();
    let built = outputs(root.path(), &store, "built", &BUILT);
    let base = tree_text(&store, None, "board.dtb", "mine", &Ctx::new()).expect("text");
    let fails = |tree: Option<&Tree>, files: &str, want: &str| {
        let err = tree_select(&store, tree, &built, files, &Ctx::new()).expect_err(files);
        assert!(err.to_string().contains(want), "{files}: {err}");
    };
    fails(
        None,
        "none.dtb",
        "the tree to select from has no file none.dtb",
    );
    let one = Artifact {
        kind: ArtifactKind::parse("file"),
        object: Store::open(&store)
            .expect("store")
            .put_file(&root.path().join("built/board.dtb"))
            .expect("put")
            .into(),
    };
    let err = tree_select(&store, None, &one, "board.dtb", &Ctx::new()).expect_err("one file");
    assert!(
        err.to_string()
            .contains("the artifact to select from is one file, not a tree"),
        "{err}"
    );
    fails(None, "  \n ", "no files to select");
    fails(
        None,
        "board.dtb other.dtb=board.dtb",
        "/board.dtb is named twice: `board.dtb` and `other.dtb=board.dtb`",
    );
    fails(None, "board.dtb=../board.dtb", "not a path inside the tree");
    fails(None, "../x.dtb", "not a path inside the tree");
    fails(
        Some(&base),
        "board.dtb",
        "/board.dtb is already a file in the tree",
    );
    fails(
        Some(&base),
        "other.dtb=board.dtb/other.dtb",
        "/board.dtb/other.dtb: its directory is a file in the tree",
    );
}
