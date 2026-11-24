//! `gaia.tree.role`: files gathered under the names of what they are.

use gaia_step::{Ctx, Store};
use gaia_tree::tree_role;
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::path::PathBuf;

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

    /// One file as an artifact.
    fn file(&self, name: &str, bytes: &[u8]) -> Artifact {
        let path = self.root.join(name);
        std::fs::write(&path, bytes).expect("write");
        let store = Store::open(&self.store).expect("store");
        Artifact {
            kind: ArtifactKind::parse("file"),
            object: store.put_file(&path).expect("put").into(),
        }
    }

    /// A directory of files as an artifact, as a kernel build gives its.
    fn tree(&self, name: &str, files: &[(&str, &[u8])]) -> Artifact {
        let dir = self.root.join(name);
        for (path, bytes) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(&path, bytes).expect("write");
        }
        let store = Store::open(&self.store).expect("store");
        Artifact {
            kind: ArtifactKind::parse("tree"),
            object: store.put_tree(&dir).expect("put").into(),
        }
    }

    fn add(
        &self,
        tree: Option<&Tree>,
        from: &Artifact,
        path: &str,
        role: &str,
    ) -> Result<Tree, String> {
        let path = path.to_string();
        tree_role(
            &self.store,
            tree,
            from,
            (!path.is_empty()).then_some(&path),
            role,
            &Ctx::new(),
        )
        .map_err(|e| e.to_string())
    }

    fn read(&self, tree: &Tree, name: &str) -> Vec<u8> {
        let store = Store::open(&self.store).expect("store");
        std::fs::read(tree.object.path_in(&store).expect("in store").join(name)).expect(name)
    }
}

#[test]
fn files_are_gathered_under_their_roles() {
    let f = Fixture::new();
    let kernel = f.tree("kernel", &[("Image", b"kernel bytes")]);
    let trees = f.tree("dtbs", &[("a.dtb", b"tree a"), ("vendor/b.dtb", b"tree b")]);
    let line = f.file("cmdline", b"console=x\n");

    // A tree of one file with no path: that file. A path names one of several.
    let one = f.add(None, &kernel, "", "kernel").expect("the one image");
    let two = f
        .add(Some(&one), &trees, "vendor/b.dtb", "device-tree")
        .expect("the named tree");
    let all = f.add(Some(&two), &line, "", "cmdline").expect("a file");
    assert_eq!(f.read(&all, "kernel"), b"kernel bytes");
    assert_eq!(f.read(&all, "device-tree"), b"tree b");
    assert_eq!(f.read(&all, "cmdline"), b"console=x\n");
    let names: Vec<&str> = all.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(names, ["cmdline", "device-tree", "kernel"]);
    assert!(all.entries.iter().all(|e| e.mode == 0o644 && e.uid == 0));
}

#[test]
fn what_cannot_be_one_file_of_one_role_is_refused() {
    let f = Fixture::new();
    let trees = f.tree("dtbs", &[("a.dtb", b"tree a"), ("b.dtb", b"tree b")]);
    let line = f.file("cmdline", b"console=x\n");

    let why = f
        .add(None, &trees, "", "device-tree")
        .expect_err("two files");
    assert!(
        why.contains(
            "the device-tree is taken from a tree of 2 files (a.dtb, b.dtb); name the one"
        ),
        "{why}"
    );
    let why = f
        .add(None, &trees, "c.dtb", "device-tree")
        .expect_err("absent");
    assert!(
        why.contains("the device-tree: no file c.dtb was built"),
        "{why}"
    );
    let why = f
        .add(None, &line, "x", "cmdline")
        .expect_err("a path in a file");
    assert!(why.contains("the cmdline is one file"), "{why}");
    let why = f
        .add(None, &line, "", "boot/cmdline")
        .expect_err("not one name");
    assert!(why.contains("is not a role: one name"), "{why}");
    // A role twice: nothing is replaced.
    let once = f.add(None, &line, "", "cmdline").expect("once");
    let why = f.add(Some(&once), &line, "", "cmdline").expect_err("twice");
    assert!(why.contains("the tree already has a cmdline"), "{why}");
}
