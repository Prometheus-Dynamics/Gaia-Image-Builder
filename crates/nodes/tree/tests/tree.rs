//! Placing, generating and merging trees.

use gaia_step::Ctx;
use gaia_store::{Store, tree_digest};
use gaia_tree::{merge_layers, tree_accounts, tree_place, tree_services};
use gaia_types::{
    Account, AccountGroup, FileEntry, Layer, Owned, Placement, Placements, Tree, owned_digest,
};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

struct Fixture {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
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

/// Stores the directory `files` (path, contents) makes as a tree.
fn stored(f: &Fixture, name: &str, files: &[(&str, &str)]) -> Tree {
    let dir = f.root.join("src").join(name);
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, text).expect("write");
    }
    std::fs::create_dir_all(&dir).expect("mkdir");
    let store = Store::open(&f.store).expect("store");
    Tree {
        object: store.put_tree(&dir).expect("put").into(),
        entries: Vec::new(),
    }
}

fn place(
    f: &Fixture,
    owner: &str,
    files: &[(&str, &str)],
    at: &[(&str, &str, Option<u32>, u32)],
) -> Layer {
    let tree = stored(f, owner, files);
    let placements = at
        .iter()
        .map(|(from, to, mode, uid)| Placement {
            from: from.to_string(),
            to: to.to_string(),
            mode: *mode,
            uid: *uid,
            gid: *uid,
        })
        .collect();
    tree_place(
        &f.store,
        owner,
        tree,
        &Placements {
            entries: placements,
        },
        &Ctx::new(),
    )
    .expect("places")
}

fn base(f: &Fixture) -> Layer {
    let tree = stored(
        f,
        "base",
        &[
            ("etc/passwd", "root:x:0:0:root:/root:/bin/sh\n"),
            ("etc/group", "root:x:0:\ndialout:x:20:\n"),
            ("etc/shadow", "root:::::::\n"),
            ("usr/bin/sh", "#!\n"),
        ],
    );
    Layer {
        owner: "os".into(),
        tree,
        replaces: Vec::new(),
    }
}

#[test]
fn a_conflict_names_both_components() {
    let f = fixture();
    let a = place(
        &f,
        "camera",
        &[("bin/server", "a")],
        &[("bin/server", "/usr/bin/server", None, 0)],
    );
    let b = place(
        &f,
        "web",
        &[("dist/server", "b")],
        &[("dist/server", "/usr/bin/server", None, 0)],
    );
    let error = merge_layers(&f.store, &[base(&f), a, b], &Ctx::new()).expect_err("conflict");
    println!("{error}");
    let text = error.to_string();
    assert!(text.contains("/usr/bin/server"), "{text}");
    assert!(text.contains("camera") && text.contains("web"), "{text}");
}

#[test]
fn a_file_over_a_base_file_is_a_conflict_unless_replaced() {
    let f = fixture();
    let a = place(
        &f,
        "shell",
        &[("sh", "new")],
        &[("sh", "/usr/bin/sh", None, 0)],
    );
    let error = merge_layers(&f.store, &[base(&f), a.clone()], &Ctx::new()).expect_err("conflict");
    println!("{error}");
    assert!(error.to_string().contains("os and shell"));
    let replacing = Layer {
        replaces: vec!["/usr/bin/sh".into()],
        ..a
    };
    let (tree, _) =
        merge_layers(&f.store, &[base(&f), replacing.clone()], &Ctx::new()).expect("replaces");
    let store = Store::open(&f.store).expect("store");
    let sh = tree.object.path_in(&store).expect("in").join("usr/bin/sh");
    assert_eq!(std::fs::read_to_string(sh).expect("read"), "new");
    let mut second = replacing.clone();
    second.owner = "busybox".into();
    let error = merge_layers(&f.store, &[base(&f), replacing, second], &Ctx::new())
        .expect_err("two replace");
    println!("{error}");
    assert!(
        error
            .to_string()
            .contains("replaced by both shell and busybox"),
        "{error}"
    );
}

#[test]
fn a_shared_directory_needs_the_same_mode_and_owner() {
    let f = fixture();
    let a = place(
        &f,
        "a",
        &[("d/x", "x")],
        &[("d", "/srv/data", Some(0o750), 1000)],
    );
    let b = place(
        &f,
        "b",
        &[("d/y", "y")],
        &[("d", "/srv/data", Some(0o755), 0)],
    );
    let error = merge_layers(&f.store, &[a.clone(), b], &Ctx::new()).expect_err("conflict");
    println!("{error}");
    assert!(
        error
            .to_string()
            .contains("750 1000:1000 in a, 755 0:0 in b")
    );
    let c = place(
        &f,
        "c",
        &[("d/z", "z")],
        &[("d", "/srv/data", Some(0o750), 1000)],
    );
    let (tree, _) = merge_layers(&f.store, &[a, c], &Ctx::new()).expect("merges");
    let data = tree
        .entries
        .iter()
        .find(|e| e.path == "srv/data")
        .expect("srv/data");
    assert_eq!((data.mode, data.uid), (0o750, 1000));
}

#[test]
fn owners_and_modes_survive_into_the_tree_digest() {
    let f = fixture();
    let layers = |mode: u32, uid: u32| {
        vec![
            base(&f),
            place(
                &f,
                "tool",
                &[("bin/tool", "t")],
                &[("bin/tool", "/usr/bin/tool", Some(mode), uid)],
            ),
        ]
    };
    let (plain, owners) = merge_layers(&f.store, &layers(0o755, 0), &Ctx::new()).expect("merges");
    let (setuid, _) = merge_layers(&f.store, &layers(0o4755, 0), &Ctx::new()).expect("merges");
    let (owned, _) = merge_layers(&f.store, &layers(0o755, 1000), &Ctx::new()).expect("merges");
    let store = Store::open(&f.store).expect("store");

    // Every path has an entry and an owner.
    let dir = plain.object.path_in(&store).expect("in store");
    let owned_list: Vec<Owned> = serde_json::from_slice(
        &std::fs::read(owners.object.path_in(&store).expect("owners")).expect("read"),
    )
    .expect("json");
    let mut on_disk = Vec::new();
    walk(&dir, "", &mut on_disk);
    let listed: Vec<&str> = owned_list.iter().map(|o| o.path.as_str()).collect();
    assert_eq!(listed, on_disk);
    assert!(owned_list.iter().all(|o| !o.owners.is_empty()));
    for o in &owned_list {
        println!(
            "{:>5o} {}:{} {:<24} {}",
            o.mode,
            o.uid,
            o.gid,
            o.path,
            o.owners.join(",")
        );
    }

    // The setuid bit is in the stored tree; the owner is in the entries.
    let mode_of = |tree: &Tree| {
        std::fs::metadata(
            tree.object
                .path_in(&store)
                .expect("in")
                .join("usr/bin/tool"),
        )
        .expect("stat")
        .permissions()
        .mode()
            & 0o7777
    };
    assert_eq!(mode_of(&setuid), 0o4755);
    let tool = |tree: &Tree| {
        tree.entries
            .iter()
            .find(|e| e.path == "usr/bin/tool")
            .cloned()
            .expect("tool")
    };
    assert_eq!(
        tool(&owned),
        FileEntry {
            path: "usr/bin/tool".into(),
            mode: 0o755,
            uid: 1000,
            gid: 1000
        }
    );
    let digests = [&plain, &setuid, &owned].map(|t| owned_digest(t).expect("digest"));
    let objects = [&plain, &setuid, &owned]
        .map(|t| tree_digest(&t.object.path_in(&store).expect("in")).expect("digest"));
    println!("plain  {} (files {})", digests[0], objects[0]);
    println!("setuid {} (files {})", digests[1], objects[1]);
    println!("uid    {} (files {})", digests[2], objects[2]);
    assert_ne!(digests[0], digests[1]);
    assert_ne!(digests[0], digests[2]);
    assert_ne!(objects[0], objects[1], "the mode is in the files");
    assert_eq!(objects[0], objects[2], "the owner is only in the entries");

    // Same layers in another store: the same digests.
    let g = fixture();
    let again = vec![
        base(&g),
        place(
            &g,
            "tool",
            &[("bin/tool", "t")],
            &[("bin/tool", "/usr/bin/tool", Some(0o755), 1000)],
        ),
    ];
    let (other, _) = merge_layers(&g.store, &again, &Ctx::new()).expect("merges");
    assert_eq!(owned_digest(&other), owned_digest(&owned));
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .expect("read")
        .map(|e| e.expect("entry"))
        .collect();
    names.sort_by_key(|e| e.file_name());
    for entry in names {
        let name = entry.file_name().into_string().expect("utf8");
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        out.push(path.clone());
        if entry.file_type().expect("type").is_dir() {
            walk(&entry.path(), &path, out);
        }
    }
    if prefix.is_empty() {
        out.sort();
    }
}

#[test]
fn accounts_and_services_are_generated_from_descriptions() {
    let f = fixture();
    let os = base(&f);
    let users = vec![
        Account {
            owner: "camera".into(),
            name: "camera".into(),
            uid: None,
            group: None,
            groups: vec!["dialout".into(), "video".into()],
            home: Some("/var/lib/camera".into()),
            shell: None,
        },
        Account {
            owner: "web".into(),
            name: "www".into(),
            uid: Some(1500),
            group: Some("video".into()),
            groups: vec![],
            home: None,
            shell: None,
        },
    ];
    let groups = vec![AccountGroup {
        owner: "camera".into(),
        name: "video".into(),
        gid: None,
    }];
    let accounts =
        tree_accounts(&f.store, os.tree.clone(), users, groups, &Ctx::new()).expect("accounts");
    let unit = "[Unit]\nDescription=camera\n[Service]\nExecStart=/usr/bin/camera\n[Install]\nWantedBy=multi-user.target\n";
    let camera = place(
        &f,
        "camera",
        &[("camera.service", unit), ("camera", "c")],
        &[
            (
                "camera.service",
                "/usr/lib/systemd/system/camera.service",
                Some(0o644),
                0,
            ),
            ("camera", "/usr/bin/camera", Some(0o755), 0),
        ],
    );
    let services = tree_services(
        &f.store,
        "camera",
        camera.tree.clone(),
        vec!["camera.service".into()],
        &Ctx::new(),
    )
    .expect("services");
    let (tree, _) =
        merge_layers(&f.store, &[os, camera, services, accounts], &Ctx::new()).expect("merges");
    let store = Store::open(&f.store).expect("store");
    let dir = tree.object.path_in(&store).expect("in");
    let read = |p: &str| std::fs::read_to_string(dir.join(p)).expect(p);
    println!(
        "--- passwd\n{}--- group\n{}--- shadow\n{}",
        read("etc/passwd"),
        read("etc/group"),
        read("etc/shadow")
    );
    assert_eq!(
        read("etc/passwd"),
        "root:x:0:0:root:/root:/bin/sh\ncamera:x:1000:1001::/var/lib/camera:/bin/false\nwww:x:1500:1000::/:/bin/false\n"
    );
    assert_eq!(
        read("etc/group"),
        "root:x:0:\ndialout:x:20:camera\nvideo:x:1000:camera\ncamera:x:1001:\n"
    );
    let link = dir.join("etc/systemd/system/multi-user.target.wants/camera.service");
    let target = std::fs::read_link(&link).expect("link");
    println!(
        "{} -> {}",
        link.strip_prefix(&dir).expect("in").display(),
        target.display()
    );
    assert_eq!(target, Path::new("/usr/lib/systemd/system/camera.service"));
    let shadow = tree
        .entries
        .iter()
        .find(|e| e.path == "etc/shadow")
        .expect("shadow");
    assert_eq!(shadow.mode, 0o600);
}

#[test]
fn a_unit_without_an_install_section_cannot_be_enabled() {
    let f = fixture();
    let tree = stored(
        &f,
        "units",
        &[(
            "usr/lib/systemd/system/oneshot.service",
            "[Service]\nExecStart=/bin/true\n",
        )],
    );
    let error = tree_services(
        &f.store,
        "boot",
        tree.clone(),
        vec!["oneshot.service".into()],
        &Ctx::new(),
    )
    .expect_err("no install");
    assert!(
        error
            .to_string()
            .contains("oneshot.service has no WantedBy"),
        "{error}"
    );
    let error = tree_services(
        &f.store,
        "boot",
        tree,
        vec!["missing.service".into()],
        &Ctx::new(),
    )
    .expect_err("missing");
    assert!(
        error
            .to_string()
            .contains("boot: no unit file for missing.service"),
        "{error}"
    );
}

/// A file in the store, as an image.
fn image(f: &Fixture, bytes: &[u8]) -> gaia_types::Image {
    let path = f.root.join("image.bin");
    std::fs::write(&path, bytes).expect("write");
    let store = Store::open(&f.store).expect("store");
    gaia_types::Image {
        kind: gaia_types::ImageKind::Filesystem,
        object: store.put_file(&path).expect("put").into(),
        size: gaia_types::Size(bytes.len() as u64),
        partitions: Vec::new(),
    }
}

fn tree_path(f: &Fixture, tree: &Tree) -> std::path::PathBuf {
    let store = Store::open(&f.store).expect("store");
    tree.object.path_in(&store).expect("stored")
}

#[test]
fn a_file_is_added_to_a_tree_and_the_rest_is_kept() {
    let f = fixture();
    let mut base = stored(
        &f,
        "data",
        &[("etc/app.conf", "a=1\n"), ("README", "data\n")],
    );
    base.entries = vec![FileEntry {
        path: "etc/app.conf".into(),
        mode: 0o600,
        uid: 7,
        gid: 7,
    }];
    let slot = image(&f, b"slot bytes");
    let out = gaia_tree::tree_file(&f.store, Some(&base), &slot, "slots/root.img", &Ctx::new())
        .expect("adds");
    let dir = tree_path(&f, &out);
    assert_eq!(
        std::fs::read(dir.join("slots/root.img")).expect("the slot"),
        b"slot bytes"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("etc/app.conf")).expect("kept"),
        "a=1\n"
    );
    assert!(dir.join("README").is_file());
    let mode = std::fs::metadata(dir.join("slots/root.img"))
        .expect("meta")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o644);
    // The tree's own entries are kept, and the file is root's.
    assert_eq!(
        out.entries,
        vec![
            FileEntry {
                path: "etc/app.conf".into(),
                mode: 0o600,
                uid: 7,
                gid: 7
            },
            FileEntry {
                path: "slots/root.img".into(),
                mode: 0o644,
                uid: 0,
                gid: 0
            },
        ]
    );
    // The same inputs give the same tree; another file gives another.
    let again = gaia_tree::tree_file(&f.store, Some(&base), &slot, "slots/root.img", &Ctx::new())
        .expect("again");
    assert_eq!(again, out);
    let other = image(&f, b"other bytes");
    let changed =
        gaia_tree::tree_file(&f.store, Some(&base), &other, "slots/root.img", &Ctx::new())
            .expect("changed");
    assert_ne!(changed.object, out.object);
}

#[test]
fn a_file_with_no_tree_is_a_tree_of_that_file() {
    let f = fixture();
    let slot = image(&f, b"slot bytes");
    let out =
        gaia_tree::tree_file(&f.store, None, &slot, "slots/root.img", &Ctx::new()).expect("adds");
    let dir = tree_path(&f, &out);
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("read")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["slots"]);
    assert!(dir.join("slots/root.img").is_file());
}

#[test]
fn a_path_the_tree_has_or_one_outside_it_is_refused() {
    let f = fixture();
    let base = stored(&f, "data", &[("slots/root.img", "old\n")]);
    let slot = image(&f, b"slot bytes");
    let error = gaia_tree::tree_file(&f.store, Some(&base), &slot, "slots/root.img", &Ctx::new())
        .expect_err("taken");
    assert!(
        error
            .to_string()
            .contains("/slots/root.img is already a file"),
        "{error}"
    );
    let error = gaia_tree::tree_file(&f.store, Some(&base), &slot, "../out.img", &Ctx::new())
        .expect_err("outside");
    println!("{error}");
}
