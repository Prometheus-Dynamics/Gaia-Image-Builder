//! `gaia.tree.assemble`: one file from several owners' parts, in order.

use daedalus_cpu_author::FanIn;
use gaia_step::{Ctx, Store, logs_dir, step_records};
use gaia_tree::{assemble, merge_layers, tree_assemble, tree_place};
use gaia_types::{FileEntry, Layer, Placement, Placements, Tree};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;

/// Stores the directory `files` (path, contents) makes as a tree.
fn stored(root: &std::path::Path, store: &str, name: &str, files: &[(&str, &str)]) -> Tree {
    let dir = root.join("src").join(name);
    std::fs::create_dir_all(&dir).expect("mkdir");
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, text).expect("write");
    }
    let opened = Store::open(store).expect("store");
    Tree {
        object: opened.put_tree(&dir).expect("put").into(),
        entries: Vec::new(),
    }
}

/// A component's fragment, placed at `to` as the wiring places it.
fn part(root: &std::path::Path, store: &str, owner: &str, to: &str, text: &str) -> Layer {
    let tree = stored(root, store, owner, &[("fragment", text)]);
    tree_place(
        store,
        owner,
        tree,
        &Placements {
            entries: vec![Placement {
                from: "fragment".into(),
                to: to.into(),
                mode: Some(0o644),
                uid: 0,
                gid: 0,
            }],
        },
        &Ctx::new(),
    )
    .expect("placed")
}

/// The parts as a fan-in takes them.
fn fan(parts: &[Layer]) -> Vec<Option<Layer>> {
    parts.iter().cloned().map(Some).collect()
}

fn order(names: &[&str]) -> String {
    serde_json::to_string(names).expect("json")
}

const OS: &str = "# the OS's\nnull root:root 666\n";
const LEMNOS: &str = "^lemnos$  root:lemnos 0660\n# a comment\n\n^i2c-[0-9]+$ root:lemnos 0660\n";
const BOARD: &str = "^camera0$ root:lemnos 0660";

#[test]
fn the_parts_are_put_together_in_the_stated_order_over_the_os_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let store = root.join("store").display().to_string();
    let mut base = stored(
        root,
        &store,
        "os",
        &[
            ("etc/mdev.conf", OS),
            ("etc/passwd", "root:x:0:0::/:/bin/sh\n"),
        ],
    );
    base.entries = vec![FileEntry {
        path: "etc/mdev.conf".into(),
        mode: 0o640,
        uid: 0,
        gid: 5,
        xattrs: Default::default(),
    }];
    let lemnos = part(root, &store, "lemnos", "/etc/mdev.conf", LEMNOS);
    let board = part(root, &store, "board", "/etc/mdev.conf", BOARD);
    let parts = [board.clone(), lemnos.clone()];
    let layer = tree_assemble(
        &store,
        base.clone(),
        "/etc/mdev.conf",
        &order(&["lemnos", "board", "os"]),
        1,
        FanIn::values(&fan(&parts)),
        &Ctx::new(),
    )
    .expect("assembled");
    let opened = Store::open(&store).expect("store");
    let file = layer
        .tree
        .object
        .path_in(&opened)
        .expect("in the store")
        .join("etc/mdev.conf");
    // In the order given, not the ports'; a newline after the part with none.
    assert_eq!(
        std::fs::read_to_string(&file).expect("read"),
        format!("{LEMNOS}{BOARD}\n{OS}")
    );
    assert_eq!(
        std::fs::metadata(&file).expect("stat").permissions().mode() & 0o7777,
        0o640,
        "the OS's mode"
    );
    assert_eq!(layer.owner, "assembled");
    assert_eq!(layer.replaces, ["etc/mdev.conf"]);
    assert_eq!(
        layer.tree.entries,
        [FileEntry {
            path: "etc/mdev.conf".into(),
            mode: 0o640,
            uid: 0,
            gid: 5,
            xattrs: Default::default(),
        }]
    );

    // Merged over the OS's tree, the assembled file is the one there.
    let os = Layer {
        owner: "os".into(),
        tree: base.clone(),
        replaces: Vec::new(),
    };
    let (merged, _) =
        merge_layers(&store, &[os.clone(), layer.clone()], &Ctx::new()).expect("merge");
    let out = merged.object.path_in(&opened).expect("in the store");
    assert_eq!(
        std::fs::read_to_string(out.join("etc/mdev.conf")).expect("read"),
        format!("{LEMNOS}{BOARD}\n{OS}")
    );
    // The fragments go to the assembly, not the merge: both would claim it.
    let err = merge_layers(&store, &[os, lemnos.clone(), board.clone()], &Ctx::new())
        .expect_err("two claims");
    assert!(err.to_string().contains("etc/mdev.conf"), "{err}");

    // Keyed on what it writes: the same parts again are looked up, another
    // order is another file.
    let again = tree_assemble(
        &store,
        base.clone(),
        "etc/mdev.conf",
        &order(&["lemnos", "board", "os"]),
        1,
        FanIn::values(&fan(&[lemnos.clone(), board.clone()])),
        &Ctx::new(),
    )
    .expect("assembled");
    assert_eq!(again, layer);
    let other = tree_assemble(
        &store,
        base.clone(),
        "etc/mdev.conf",
        &order(&["os", "lemnos", "board"]),
        1,
        FanIn::values(&fan(&parts)),
        &Ctx::new(),
    )
    .expect("assembled");
    assert_ne!(other.tree.object, layer.tree.object);
    let made = step_records(&logs_dir(&opened))
        .into_iter()
        .filter(|r| r.step == "gaia.tree.assemble" && r.made)
        .count();
    assert_eq!(made, 2);

    // With no OS file: root's 0644, and nothing replaced.
    let bare = stored(root, &store, "bare", &[("etc/passwd", "x\n")]);
    let layer = tree_assemble(
        &store,
        bare,
        "/etc/mdev.conf",
        &order(&["lemnos"]),
        1,
        FanIn::values(&fan(&[lemnos])),
        &Ctx::new(),
    )
    .expect("assembled");
    assert!(layer.replaces.is_empty());
    assert_eq!(layer.tree.entries[0].mode, 0o644);
    assert_eq!(layer.tree.entries[0].gid, 0);
}

#[test]
fn a_part_missing_from_the_order_or_the_tree_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let store = root.join("store").display().to_string();
    let base = stored(root, &store, "os", &[("etc/mdev.conf", OS)]);
    let bare = stored(root, &store, "bare", &[("etc/passwd", "x\n")]);
    let lemnos = part(root, &store, "lemnos", "/etc/mdev.conf", LEMNOS);
    let elsewhere = part(root, &store, "elsewhere", "/etc/other.conf", BOARD);
    let named_os = part(root, &store, "os", "/etc/mdev.conf", BOARD);
    let fails = |base: &Tree, names: &[&str], parts: &[Layer], key: i64, want: &str| {
        let err = tree_assemble(
            &store,
            base.clone(),
            "/etc/mdev.conf",
            &order(names),
            key,
            FanIn::values(&fan(parts)),
            &Ctx::new(),
        )
        .expect_err(want);
        assert!(err.to_string().contains(want), "{want}: {err}");
    };
    let one = std::slice::from_ref(&lemnos);
    fails(
        &base,
        &["lemnos"],
        one,
        1,
        "the OS's tree has /etc/mdev.conf, and its order does not name `os`",
    );
    fails(
        &bare,
        &["lemnos", "os"],
        one,
        1,
        "names `os`, and the OS's tree has no /etc/mdev.conf",
    );
    fails(
        &base,
        &["os"],
        one,
        1,
        "lemnos gives a part of /etc/mdev.conf, and its order does not name it",
    );
    fails(
        &base,
        &["os", "lemnos", "board"],
        one,
        1,
        "names board, which gives no part of it",
    );
    fails(
        &base,
        &["os", "lemnos", "os"],
        one,
        1,
        "`os` is named twice",
    );
    fails(
        &base,
        &["os", "elsewhere"],
        &[elsewhere],
        1,
        "elsewhere's part has no file /etc/mdev.conf",
    );
    fails(
        &base,
        &["os", "lemnos"],
        &[lemnos.clone(), lemnos.clone()],
        1,
        "lemnos gives two parts of /etc/mdev.conf",
    );
    fails(
        &base,
        &["os"],
        &[named_os],
        1,
        "owned by `os`, the OS's name",
    );
    fails(
        &base,
        &["os", "lemnos"],
        one,
        -1,
        "rule_key -1 is not a field number",
    );
    fails(&base, &[], &[], 1, "no parts in its order");
    let a_dir = stored(root, &store, "dir", &[("etc/mdev.conf/inner", "x\n")]);
    fails(
        &a_dir,
        &["os", "lemnos"],
        one,
        1,
        "/etc/mdev.conf in the OS's tree is not a file",
    );
}

fn texts(parts: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
    parts
        .iter()
        .map(|(owner, text)| (owner.to_string(), text.as_bytes().to_vec()))
        .collect()
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| name.to_string()).collect()
}

#[test]
fn two_rules_of_one_name_are_refused_naming_both_owners() {
    let both = texts(&[
        ("lemnos", "# x\n^video[0-9]+$ root:lemnos 0660\n"),
        ("board", "\n\n  ^video[0-9]+$   root:video 0600\n"),
    ]);
    let err =
        assemble("etc/mdev.conf", &names(&["lemnos", "board"]), &both, 1).expect_err("a duplicate");
    assert_eq!(
        err,
        "/etc/mdev.conf: the rule `^video[0-9]+$` is in lemnos (line 2) and in board (line 3)"
    );
    // By whole rules, the two differ; the same rule twice does not.
    assert!(assemble("etc/mdev.conf", &names(&["lemnos", "board"]), &both, 0).is_ok());
    let same = texts(&[("a", "x  y\n"), ("b", "x y\n")]);
    let err = assemble("f", &names(&["a", "b"]), &same, 0).expect_err("a duplicate");
    assert!(
        err.contains("the rule `x y` is in a (line 1) and in b (line 1)"),
        "{err}"
    );
    // By the second field.
    let second = texts(&[("a", "one /mnt\n"), ("b", "two /mnt\n")]);
    let err = assemble("f", &names(&["a", "b"]), &second, 2).expect_err("a duplicate");
    assert!(err.contains("the rule `/mnt`"), "{err}");
    // In one part, too.
    let twice = texts(&[("a", "^x$ a\n^x$ b\n")]);
    let err = assemble("f", &names(&["a"]), &twice, 1).expect_err("a duplicate");
    assert!(err.contains("is in a (line 1) and in a (line 2)"), "{err}");
    // Comments and blank lines are not rules; a rule with fewer fields is
    // named whole.
    let fine = texts(&[
        ("a", "# ^x$\n\n   \n#^x$\nshort\n"),
        ("b", "^x$ b\nshort\t\n"),
    ]);
    let err = assemble("f", &names(&["a", "b"]), &fine, 3).expect_err("short twice");
    assert!(
        err.contains("the rule `short` is in a (line 5) and in b (line 2)"),
        "{err}"
    );
    let ok = texts(&[("a", "# ^x$\n#^x$\n"), ("b", "^x$ b\n")]);
    assert_eq!(
        assemble("f", &names(&["a", "b"]), &ok, 1).expect("fine"),
        b"# ^x$\n#^x$\n^x$ b\n"
    );
    // An empty part adds nothing, not a newline; text that is not UTF-8 is refused.
    let empty = texts(&[("a", ""), ("b", "z\n")]);
    assert_eq!(
        assemble("f", &names(&["a", "b"]), &empty, 1).expect("fine"),
        b"z\n"
    );
    let mut binary = texts(&[("a", "")]);
    binary.insert("a".into(), vec![0xff, b'\n']);
    let err = assemble("f", &names(&["a"]), &binary, 1).expect_err("binary");
    assert!(err.contains("a's part of /f is not UTF-8 text"), "{err}");
}
