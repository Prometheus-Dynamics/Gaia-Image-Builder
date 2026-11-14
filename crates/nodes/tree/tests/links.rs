//! `gaia.tree.links`: symbolic links as a layer, and what a merge makes of
//! a link and a real directory at one path.

use gaia_step::Ctx;
use gaia_store::Store;
use gaia_tree::{merge_layers, tree_links, tree_place};
use gaia_types::{Layer, Placement, Placements, Tree};
use std::path::Path;

fn store() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = dir.path().join("store").display().to_string();
    (dir, store)
}

#[test]
fn each_path_is_a_link_to_its_target_as_written() {
    let (_dir, store) = store();
    let layer = tree_links(
        &store,
        "daemon",
        r#"[["/etc/board", "/run/board"], ["/usr/lib/x/current", "../y"]]"#,
        &Ctx::new(),
    )
    .expect("links");
    assert_eq!(layer.owner, "daemon");
    let opened = Store::open(&store).expect("store");
    let tree = layer.tree.object.path_in(&opened).expect("in the store");
    // The target is kept as given, whether or not it is in the image.
    assert_eq!(
        std::fs::read_link(tree.join("etc/board")).expect("a link"),
        Path::new("/run/board")
    );
    assert_eq!(
        std::fs::read_link(tree.join("usr/lib/x/current")).expect("a link"),
        Path::new("../y")
    );
    assert!(
        std::fs::symlink_metadata(tree.join("etc"))
            .expect("stat")
            .is_dir()
    );
}

#[test]
fn a_path_twice_a_parent_and_a_child_and_an_empty_target_are_refused() {
    let (_dir, store) = store();
    let run = |links: &str| {
        tree_links(&store, "daemon", links, &Ctx::new())
            .expect_err("refused")
            .to_string()
    };
    let why = run(r#"[["/etc/board", "/run/a"], ["/etc/board", "/run/b"]]"#);
    assert!(
        why.contains("/etc/board and /etc/board cannot both be links"),
        "{why}"
    );
    let why = run(r#"[["/etc/board", "/run/a"], ["/etc/board/security", "/run/b"]]"#);
    assert!(why.contains("or its parent"), "{why}");
    let why = run(r#"[["/etc/board", ""]]"#);
    assert!(why.contains("/etc/board is a link to nothing"), "{why}");
    let why = run(r#"[["../etc/board", "/run/a"]]"#);
    assert!(why.contains("not a path inside the tree"), "{why}");
    let why = run("not json");
    assert!(why.contains("not a list of [path, target] pairs"), "{why}");
    let why = tree_links(&store, "", "[]", &Ctx::new())
        .expect_err("no owner")
        .to_string();
    assert!(why.contains("a layer needs an owner"), "{why}");
}

/// A component that puts a real file under a path another makes a link: the
/// image cannot have both, and the merge says who.
#[test]
fn a_link_and_a_real_directory_at_one_path_do_not_merge() {
    let (dir, store) = store();
    let links = tree_links(
        &store,
        "daemon",
        r#"[["/etc/board", "/run/board"]]"#,
        &Ctx::new(),
    )
    .expect("links");
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).expect("mkdir");
    std::fs::write(src.join("security"), "off\n").expect("write");
    let opened = Store::open(&store).expect("store");
    let files: Layer = tree_place(
        &store,
        "other",
        Tree {
            object: opened.put_tree(&src).expect("put").into(),
            entries: Vec::new(),
        },
        &Placements {
            entries: vec![Placement {
                from: "security".into(),
                to: "/etc/board/security".into(),
                mode: Some(0o644),
                uid: 0,
                gid: 0,
            }],
        },
        &Ctx::new(),
    )
    .expect("place");
    let why = merge_layers(&store, &[links.clone(), files], &Ctx::new())
        .expect_err("a link and a directory")
        .to_string();
    println!("{why}");
    assert!(why.contains("etc/board"), "{why}");
    assert!(why.contains("daemon") && why.contains("other"), "{why}");
    // Alone, the link is in the merged tree as a link.
    let merged = merge_layers(&store, &[links], &Ctx::new()).expect("merges");
    let tree = merged.0.object.path_in(&opened).expect("in the store");
    assert_eq!(
        std::fs::read_link(tree.join("etc/board")).expect("a link"),
        Path::new("/run/board")
    );
}
