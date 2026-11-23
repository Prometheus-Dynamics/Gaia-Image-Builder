//! Placing, generating and merging trees.

use gaia_step::Ctx;
use gaia_store::{Store, tree_digest};
use gaia_tree::{enable_units, merge_layers, tree_accounts, tree_place, undeclared};
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
            gid: 1000,
            xattrs: Default::default()
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

/// `stored`, with `links` (path, target) as symbolic links.
fn stored_with_links(
    f: &Fixture,
    name: &str,
    files: &[(&str, &str)],
    links: &[(&str, &str)],
) -> Tree {
    let dir = f.root.join("src").join(name);
    for (path, to) in links {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::os::unix::fs::symlink(to, &path).expect("link");
    }
    stored(f, name, files)
}

fn layer(owner: &str, tree: Tree) -> Layer {
    Layer {
        owner: owner.into(),
        tree,
        replaces: Vec::new(),
    }
}

fn user(
    owner: &str,
    name: &str,
    uid: Option<u32>,
    group: Option<&str>,
    groups: &[&str],
) -> Account {
    Account {
        owner: owner.into(),
        name: name.into(),
        uid,
        group: group.map(str::to_string),
        groups: groups.iter().map(|g| g.to_string()).collect(),
        home: None,
        shell: None,
    }
}

fn group(owner: &str, name: &str, gid: Option<u32>) -> AccountGroup {
    AccountGroup {
        owner: owner.into(),
        name: name.into(),
        gid,
    }
}

fn accounts(
    f: &Fixture,
    base: &Layer,
    users: &[Account],
    groups: &[AccountGroup],
) -> Result<Layer, String> {
    tree_accounts(
        &f.store,
        base.tree.clone(),
        &serde_json::to_string(users).expect("json"),
        &serde_json::to_string(groups).expect("json"),
        &Ctx::new(),
    )
    .map_err(|e| e.to_string())
}

fn units(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(o, u)| (o.to_string(), u.to_string()))
        .collect()
}

/// A merged tree's file.
fn read_in(f: &Fixture, tree: &Tree, path: &str) -> String {
    let store = Store::open(&f.store).expect("store");
    let dir = tree.object.path_in(&store).expect("in");
    std::fs::read_to_string(dir.join(path)).expect(path)
}

#[test]
fn accounts_and_services_are_generated_from_descriptions() {
    let f = fixture();
    let os = base(&f);
    let mut camera_user = user("camera", "camera", None, None, &["dialout", "video"]);
    camera_user.home = Some("/var/lib/camera".into());
    let users = vec![
        camera_user,
        user("web", "www", Some(1500), Some("video"), &[]),
    ];
    let groups = vec![group("camera", "video", None)];
    let accounts = accounts(&f, &os, &users, &groups).expect("accounts");
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
    let services = enable_units(
        &f.store,
        &[os.clone(), camera.clone()],
        &units(&[("camera", "camera.service")]),
        &Ctx::new(),
    )
    .expect("services");
    let (tree, _) =
        merge_layers(&f.store, &[os, camera, services, accounts], &Ctx::new()).expect("merges");
    let read = |p: &str| read_in(&f, &tree, p);
    println!(
        "--- passwd\n{}--- group\n{}--- shadow\n{}",
        read("etc/passwd"),
        read("etc/group"),
        read("etc/shadow")
    );
    // Groups by name, then users by name, from 100: video 100, camera's
    // uid 100, its own group 101 (100 is video's).
    assert_eq!(
        read("etc/passwd"),
        "root:x:0:0:root:/root:/bin/sh\ncamera:x:100:101::/var/lib/camera:/bin/false\nwww:x:1500:100::/:/bin/false\n"
    );
    assert_eq!(
        read("etc/group"),
        "root:x:0:\ndialout:x:20:camera\nvideo:x:100:camera\ncamera:x:101:\n"
    );
    assert_eq!(
        read("etc/shadow"),
        "root:::::::\ncamera:*:::::::\nwww:*:::::::\n"
    );
    let store = Store::open(&f.store).expect("store");
    let dir = tree.object.path_in(&store).expect("in");
    let link = dir.join("etc/systemd/system/multi-user.target.wants/camera.service");
    let target = std::fs::read_link(&link).expect("link");
    assert_eq!(target, Path::new("/usr/lib/systemd/system/camera.service"));
    let shadow = tree
        .entries
        .iter()
        .find(|e| e.path == "etc/shadow")
        .expect("shadow");
    assert_eq!(shadow.mode, 0o600);
}

/// The ids do not depend on the order components are listed in.
#[test]
fn ids_are_the_same_in_any_order() {
    let f = fixture();
    let os = base(&f);
    let users = vec![
        user("b", "zeta", None, None, &["shared"]),
        user("a", "alpha", None, Some("shared"), &[]),
        user("c", "mid", Some(901), None, &[]),
    ];
    let groups = vec![group("a", "shared", None), group("c", "late", Some(950))];
    let files = |users: &[Account], groups: &[AccountGroup]| {
        let layer = accounts(&f, &os, users, groups).expect("accounts");
        ["etc/passwd", "etc/group", "etc/shadow"].map(|p| read_in(&f, &layer.tree, p))
    };
    let forward = files(&users, &groups);
    let mut users_back = users.clone();
    users_back.reverse();
    let mut groups_back = groups.clone();
    groups_back.reverse();
    assert_eq!(forward, files(&users_back, &groups_back));
    println!("{}{}", forward[0], forward[1]);
    assert_eq!(
        forward[0],
        "root:x:0:0:root:/root:/bin/sh\nalpha:x:100:100::/:/bin/false\nmid:x:901:901::/:/bin/false\nzeta:x:101:101::/:/bin/false\n"
    );
}

#[test]
fn a_fixed_id_another_name_has_or_a_missing_group_is_refused() {
    let f = fixture();
    let os = base(&f);
    for (users, groups, want) in [
        (
            vec![user("a", "svc", Some(0), None, &[])],
            vec![],
            "user svc in a: uid 0 is the base's user root",
        ),
        (
            vec![
                user("a", "one", Some(900), None, &[]),
                user("b", "two", Some(900), None, &[]),
            ],
            vec![],
            "uid 900: user one in a and user two in b",
        ),
        (
            vec![],
            vec![group("a", "serial", Some(20))],
            "group serial in a: gid 20 is the base's group dialout",
        ),
        (
            vec![],
            vec![group("a", "one", Some(905)), group("b", "two", Some(905))],
            "gid 905: group one in a and group two in b",
        ),
        (
            vec![user("a", "svc", None, None, &["nosuch"])],
            vec![],
            "user svc in a: no group nosuch",
        ),
        (
            vec![user("a", "svc", None, Some("nosuch"), &[])],
            vec![],
            "user svc in a: no group nosuch",
        ),
        (
            vec![
                user("a", "svc", Some(901), None, &[]),
                user("b", "svc", Some(902), None, &[]),
            ],
            vec![],
            "user svc: declared differently by a and b",
        ),
    ] {
        let error = accounts(&f, &os, &users, &groups).expect_err(want);
        assert!(error.contains(want), "{error}");
    }
    let gshadow = layer(
        "os",
        stored(
            &f,
            "gshadow",
            &[
                ("etc/passwd", "root:x:0:0::/:/bin/sh\n"),
                ("etc/gshadow", "root:*::\n"),
            ],
        ),
    );
    let error = accounts(&f, &gshadow, &[], &[group("a", "g", None)]).expect_err("gshadow");
    assert!(error.contains("/etc/gshadow"), "{error}");
}

const SYSTEM: &str = "usr/lib/systemd/system";

/// `Alias=`, `Also=` and instances, a link the OS's presets made already,
/// two components enabling one unit, and a unit reached through an
/// absolute link that is the tree's, not the host's.
#[test]
fn enabling_follows_install_like_systemctl() {
    let f = fixture();
    let os = layer(
        "os",
        stored_with_links(
            &f,
            "units",
            &[
                (
                    &format!("{SYSTEM}/net.service"),
                    "[Service]\nExecStart=/bin/net\n[Install]\nWantedBy=multi-user.target\nAlias=net-alias.service\n# Also=ignored.service\nAlso=net.socket\n",
                ),
                (
                    &format!("{SYSTEM}/net.socket"),
                    "[Install]\nWantedBy=sockets.target\n",
                ),
                (
                    &format!("{SYSTEM}/getty@.service"),
                    "[Install]\nWantedBy=getty.target\n",
                ),
                (
                    "opt/units/far.service",
                    "[Install]\nRequiredBy=far.target\nUpheldBy=up.target\n",
                ),
            ],
            &[
                (
                    "etc/systemd/system/sockets.target.wants/net.socket",
                    "/usr/lib/systemd/system/net.socket",
                ),
                (
                    &format!("{SYSTEM}/far.service"),
                    "/opt/gaia-test-not-on-the-host/../units/far.service",
                ),
            ],
        ),
    );
    let layer = enable_units(
        &f.store,
        std::slice::from_ref(&os),
        &units(&[
            ("a", "net.service"),
            ("b", "getty@ttyGS0.service"),
            ("b", "net.service"),
            ("c", "far.service"),
        ]),
        &Ctx::new(),
    )
    .expect("enables");
    let mut found = Vec::new();
    let store = Store::open(&f.store).expect("store");
    let dir = layer.tree.object.path_in(&store).expect("in");
    walk(&dir, "", &mut found);
    let links: Vec<String> = found
        .iter()
        .filter_map(|p| {
            let to = std::fs::read_link(dir.join(p)).ok()?;
            Some(format!("{p} -> {}", to.display()))
        })
        .collect();
    println!("{links:#?}");
    assert_eq!(
        links,
        [
            "etc/systemd/system/far.target.requires/far.service -> /usr/lib/systemd/system/far.service",
            "etc/systemd/system/getty.target.wants/getty@ttyGS0.service -> /usr/lib/systemd/system/getty@.service",
            "etc/systemd/system/multi-user.target.wants/net.service -> /usr/lib/systemd/system/net.service",
            "etc/systemd/system/net-alias.service -> /usr/lib/systemd/system/net.service",
            "etc/systemd/system/up.target.upholds/far.service -> /usr/lib/systemd/system/far.service",
        ]
    );
    merge_layers(&f.store, &[os, layer], &Ctx::new()).expect("merges");

    // A directory on the way that is an absolute link: from the tree's
    // root, so the unit is found where the image will have it.
    let moved = self::layer(
        "os",
        stored_with_links(
            &f,
            "moved",
            &[(
                "opt/sd/system/deep.service",
                "[Install]\nWantedBy=multi-user.target\n",
            )],
            &[("usr/lib/systemd", "/opt/sd")],
        ),
    );
    let deep = enable_units(
        &f.store,
        &[moved],
        &units(&[("d", "deep.service")]),
        &Ctx::new(),
    )
    .expect("enables through the link");
    let dir = deep.tree.object.path_in(&store).expect("in");
    assert_eq!(
        std::fs::read_link(dir.join("etc/systemd/system/multi-user.target.wants/deep.service"))
            .expect("link"),
        Path::new("/usr/lib/systemd/system/deep.service")
    );
}

#[test]
fn masked_absent_and_installless_units_are_refused() {
    let f = fixture();
    let os = layer(
        "os",
        stored_with_links(
            &f,
            "refused",
            &[
                (
                    &format!("{SYSTEM}/masked.service"),
                    "[Install]\nWantedBy=multi-user.target\n",
                ),
                (
                    &format!("{SYSTEM}/oneshot.service"),
                    "[Service]\nExecStart=/bin/true\n",
                ),
                (
                    &format!("{SYSTEM}/taken.service"),
                    "[Install]\nWantedBy=multi-user.target\n",
                ),
                (
                    &format!("{SYSTEM}/spec@.service"),
                    "[Install]\nWantedBy=%i.target\n",
                ),
                (
                    &format!("{SYSTEM}/tmpl@.service"),
                    "[Install]\nWantedBy=multi-user.target\n",
                ),
            ],
            &[
                ("etc/systemd/system/masked.service", "/dev/null"),
                (
                    "etc/systemd/system/multi-user.target.wants/taken.service",
                    "/usr/lib/systemd/system/other.service",
                ),
            ],
        ),
    );
    for (unit, want) in [
        (
            "masked.service",
            "a: masked.service is masked: /etc/systemd/system/masked.service is a link to /dev/null",
        ),
        (
            "missing.service",
            "a: missing.service has no unit file in the tree",
        ),
        (
            "oneshot.service",
            "a: oneshot.service has no [Install] entry",
        ),
        (
            "taken.service",
            "taken.service: /etc/systemd/system/multi-user.target.wants/taken.service is already in the tree, a link to /usr/lib/systemd/system/other.service",
        ),
        (
            "spec@x.service",
            "a: spec@x.service has WantedBy=%i.target, a specifier",
        ),
        ("tmpl@.service", "a: tmpl@.service is a template"),
        ("../x.service", "a: ../x.service is not a unit name"),
    ] {
        let error = enable_units(
            &f.store,
            std::slice::from_ref(&os),
            &units(&[("a", unit)]),
            &Ctx::new(),
        )
        .expect_err(unit)
        .to_string();
        assert!(error.contains(want), "{error}");
    }
}

/// A component's sysusers.d and preset files against what the components
/// state: each gap a warning line; the OS's own files are not read.
#[test]
fn sysusers_and_presets_no_component_states_are_named() {
    let f = fixture();
    let os = layer(
        "os",
        stored(
            &f,
            "os-presets",
            &[
                ("usr/lib/sysusers.d/dbus.conf", "u dbus - -\n"),
                (
                    "usr/lib/systemd/system-preset/90-systemd.preset",
                    "enable getty@.service\nenable foo.service\n",
                ),
            ],
        ),
    );
    let app = layer(
        "app",
        stored(
            &f,
            "app-presets",
            &[
                (
                    "usr/lib/sysusers.d/app.conf",
                    "# a comment\nu app 901 \"App\" /var/lib/app\nu! ghost -\ng appgroup -\ng nogroup -\nm app dialout\nm app video\nr - 500-900\n",
                ),
                (
                    "usr/lib/systemd/system-preset/80-app.preset",
                    "enable app.service\nenable missing.service\nenable glob-*.service\ndisable other.service\n",
                ),
            ],
        ),
    );
    let mut app_user = user("app", "app", Some(902), None, &["dialout"]);
    app_user.home = Some("/var/lib/app".into());
    let gaps = undeclared(
        &f.store,
        &[os, app],
        &units(&[("app", "app.service")]),
        &[app_user],
        &[group("app", "appgroup", None)],
    )
    .expect("reads");
    println!("{gaps:#?}");
    assert_eq!(
        gaps,
        [
            "app: /usr/lib/sysusers.d/app.conf gives user app uid 901, and app's [[users]] 902",
            "app: /usr/lib/sysusers.d/app.conf has user ghost, which no component's [[users]] states",
            "app: /usr/lib/sysusers.d/app.conf has group nogroup, which no component's [[groups]] or [[users]] states",
            "app: /usr/lib/sysusers.d/app.conf puts app in group video, which no component's [[users]] does",
            "app: /usr/lib/systemd/system-preset/80-app.preset enables missing.service, which no component's [[services]] enables",
        ]
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
        xattrs: Default::default(),
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
                gid: 7,
                xattrs: Default::default()
            },
            FileEntry {
                path: "slots/root.img".into(),
                mode: 0o644,
                uid: 0,
                gid: 0,
                xattrs: Default::default()
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

/// An OS layer whose `/root` is root's alone, as an OS's own tables say.
fn os_with_private_root(f: &Fixture) -> Layer {
    let mut tree = stored(
        f,
        "os-root",
        &[("root/.profile", "x\n"), ("etc/hostname", "h\n")],
    );
    tree.entries = vec![FileEntry {
        path: "root".into(),
        mode: 0o700,
        uid: 0,
        gid: 0,
        xattrs: Default::default(),
    }];
    Layer {
        owner: "os".into(),
        tree,
        replaces: Vec::new(),
    }
}

fn mode_of(entries: &[FileEntry], path: &str) -> Option<u32> {
    entries.iter().find(|e| e.path == path).map(|e| e.mode)
}

#[test]
fn a_chosen_directory_mode_stands_over_a_layer_that_only_puts_files_under_it() {
    let f = fixture();
    let key = || {
        place(
            &f,
            "ssh-key",
            &[("authorized_keys", "ssh-ed25519 AAAA\n")],
            &[(
                "authorized_keys",
                "/root/.ssh/authorized_keys",
                Some(0o600),
                0,
            )],
        )
    };
    // The component's layer has /root only as the parent of its file. In
    // either order, the OS's 0700 is what the merged tree has.
    for layers in [
        vec![os_with_private_root(&f), key()],
        vec![key(), os_with_private_root(&f)],
    ] {
        let (tree, _) = merge_layers(&f.store, &layers, &Ctx::new()).expect("merges");
        assert_eq!(
            mode_of(&tree.entries, "root"),
            Some(0o700),
            "{:?}",
            tree.entries
        );
        assert_eq!(
            mode_of(&tree.entries, "root/.ssh/authorized_keys"),
            Some(0o600)
        );
        let store = Store::open(&f.store).expect("store");
        let dir = tree.object.path_in(&store).expect("stored");
        assert!(dir.join("root/.profile").is_file() && dir.join("root/.ssh").is_dir());
    }
}

#[test]
fn two_layers_that_each_choose_a_directorys_mode_still_conflict() {
    let f = fixture();
    let mut other = stored(&f, "other", &[("root/notes", "n\n")]);
    other.entries = vec![FileEntry {
        path: "root".into(),
        mode: 0o750,
        uid: 0,
        gid: 0,
        xattrs: Default::default(),
    }];
    let other = Layer {
        owner: "tools".into(),
        tree: other,
        replaces: Vec::new(),
    };
    let error = merge_layers(&f.store, &[os_with_private_root(&f), other], &Ctx::new())
        .expect_err("two choices");
    let text = error.to_string();
    println!("{text}");
    assert!(
        text.contains("/root: 700 0:0 in os, 750 0:0 in tools"),
        "{text}"
    );
}
