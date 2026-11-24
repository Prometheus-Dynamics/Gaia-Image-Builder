//! `gaia.tree.text`: a text file written from a setting, byte for byte.

use gaia_step::{Ctx, Store, logs_dir, step_records};
use gaia_tree::tree_text;

#[test]
fn the_text_is_written_as_given_and_nothing_is_replaced() {
    let root = tempfile::tempdir().expect("tempdir");
    let store = root.path().join("store").display().to_string();
    let config = "# a board\narm_64bit=1\ninitramfs boot.cpio followkernel\n";
    let one = tree_text(&store, None, "/config.txt", config, &Ctx::new()).expect("text");
    let two = tree_text(
        &store,
        Some(&one),
        "sub/cmdline.txt",
        "rdinit=/init",
        &Ctx::new(),
    )
    .expect("text");
    let opened = Store::open(&store).expect("store");
    let out = two.object.path_in(&opened).expect("in the store");
    assert_eq!(
        std::fs::read_to_string(out.join("config.txt")).expect("read"),
        config
    );
    assert_eq!(
        std::fs::read_to_string(out.join("sub/cmdline.txt")).expect("read"),
        "rdinit=/init",
        "no newline added"
    );
    let paths: Vec<&str> = two.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, ["config.txt", "sub/cmdline.txt"]);
    assert!(two.entries.iter().all(|e| e.mode == 0o644 && e.uid == 0));

    // The same text: the same step. Another text: another tree.
    let again = tree_text(&store, None, "config.txt", config, &Ctx::new()).expect("text");
    assert_eq!(again, one);
    let other = tree_text(&store, None, "config.txt", "arm_64bit=0\n", &Ctx::new()).expect("text");
    assert_ne!(other.object, one.object);
    let made = step_records(&logs_dir(&opened))
        .into_iter()
        .filter(|r| r.step == "gaia.tree.text" && r.made)
        .count();
    assert_eq!(made, 3);

    let fails = |tree, path: &str, text: &str, want: &str| {
        let err = tree_text(&store, tree, path, text, &Ctx::new()).expect_err(path);
        assert!(err.to_string().contains(want), "{path}: {err}");
    };
    fails(None, "config.txt", "", "the text for /config.txt is empty");
    fails(None, "../config.txt", "x", "not a path inside the tree");
    fails(
        Some(&one),
        "config.txt",
        "x",
        "/config.txt is already a file in the tree",
    );
    fails(
        Some(&one),
        "config.txt/inner",
        "x",
        "/config.txt/inner: its directory is a file in the tree",
    );
}
