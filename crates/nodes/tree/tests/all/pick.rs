//! `gaia.tree.pick`: one file of a tree as its own tree.

use gaia_step::{Ctx, Store, logs_dir, step_records};
use gaia_tree::tree_pick;
use gaia_types::Tree;
use std::path::Path;

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

#[test]
fn the_picked_tree_changes_only_with_the_file() {
    let root = tempfile::tempdir().expect("tempdir");
    let store = root.path().join("store").display().to_string();
    let files = [
        ("contracts/k.toml", "version = 1\n"),
        ("src/main.rs", "one"),
    ];
    let first = stored(root.path(), &store, "a", &files);
    let picked = tree_pick(&store, first, "contracts/k.toml", &Ctx::new()).expect("pick");
    let opened = Store::open(&store).expect("store");
    let out = picked.object.path_in(&opened).expect("in the store");
    assert_eq!(
        std::fs::read_to_string(out.join("contracts/k.toml")).expect("read"),
        "version = 1\n"
    );
    assert!(!out.join("src").exists(), "nothing but the file");

    // Another file of the source changes: the same picked tree.
    let files = [
        ("contracts/k.toml", "version = 1\n"),
        ("src/main.rs", "two"),
    ];
    let second = stored(root.path(), &store, "b", &files);
    let again = tree_pick(&store, second.clone(), "/contracts/k.toml", &Ctx::new()).expect("pick");
    assert_eq!(again, picked);

    let err = tree_pick(&store, second.clone(), "contracts/none.toml", &Ctx::new())
        .expect_err("no such file");
    assert!(
        err.to_string()
            .contains("the tree has no file contracts/none.toml"),
        "{err}"
    );
    let err = tree_pick(&store, second, "../k.toml", &Ctx::new()).expect_err("outside");
    assert!(
        err.to_string().contains("not a path inside the tree"),
        "{err}"
    );
    let made = step_records(&logs_dir(&opened))
        .into_iter()
        .filter(|r| r.step == "gaia.tree.pick" && r.made)
        .count();
    assert_eq!(made, 2, "one per source tree");
}
