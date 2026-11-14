//! A merge through a directory link: one layer has `/lib` as a link to
//! `usr/lib` (a merged `/usr`), another writes under `/lib`.

use gaia_step::Ctx;
use gaia_store::Store;
use gaia_tree::merge_layers;
use gaia_types::{Layer, Tree};
use std::path::Path;

struct Fixture {
    dir: tempfile::TempDir,
    store: String,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = dir.path().join("store").display().to_string();
        Self { dir, store }
    }

    /// A layer of `files` (path, contents) and `links` (path, target).
    fn layer(&self, owner: &str, files: &[(&str, &str)], links: &[(&str, &str)]) -> Layer {
        let root = self.dir.path().join(owner);
        std::fs::create_dir_all(&root).expect("mkdir");
        for (path, text) in files {
            let at = root.join(path);
            std::fs::create_dir_all(at.parent().expect("parent")).expect("mkdir");
            std::fs::write(at, text).expect("write");
        }
        for (path, target) in links {
            let at = root.join(path);
            std::fs::create_dir_all(at.parent().expect("parent")).expect("mkdir");
            std::os::unix::fs::symlink(target, at).expect("link");
        }
        let store = Store::open(&self.store).expect("store");
        Layer {
            owner: owner.into(),
            tree: Tree {
                object: store.put_tree(&root).expect("put").into(),
                entries: Vec::new(),
            },
            replaces: Vec::new(),
        }
    }

    fn merged(&self, layers: &[Layer]) -> Result<std::path::PathBuf, String> {
        let (tree, _) =
            merge_layers(&self.store, layers, &Ctx::new()).map_err(|e| e.to_string())?;
        let store = Store::open(&self.store).expect("store");
        Ok(tree.object.path_in(&store).expect("in the store"))
    }
}

#[test]
fn a_path_under_a_linked_directory_goes_where_the_link_points() {
    let f = Fixture::new();
    let os = f.layer(
        "os",
        &[("usr/lib/libc.so", "c"), ("usr/bin/sh", "sh")],
        &[("lib", "usr/lib"), ("bin", "usr/bin")],
    );
    let kernel = f.layer("kernel", &[("lib/modules/7.0/a.ko", "module")], &[]);
    let tree = f.merged(&[os, kernel]).expect("merges");
    // One place, reached both ways; the link is still a link.
    assert_eq!(
        std::fs::read_to_string(tree.join("usr/lib/modules/7.0/a.ko")).expect("read"),
        "module"
    );
    assert_eq!(
        std::fs::read_link(tree.join("lib")).expect("a link"),
        Path::new("usr/lib")
    );
    assert_eq!(
        std::fs::read_to_string(tree.join("lib/modules/7.0/a.ko")).expect("through the link"),
        "module"
    );
    assert!(tree.join("usr/lib/libc.so").is_file());
}

#[test]
fn an_absolute_link_and_a_chain_of_links_are_followed() {
    let f = Fixture::new();
    let os = f.layer(
        "os",
        &[("usr/lib/keep", "k")],
        &[("lib", "/usr/lib"), ("lib64", "lib")],
    );
    let other = f.layer("other", &[("lib64/x", "x")], &[]);
    let tree = f.merged(&[os, other]).expect("merges");
    assert_eq!(
        std::fs::read_to_string(tree.join("usr/lib/x")).expect("read"),
        "x"
    );
}

#[test]
fn a_file_both_layers_have_through_the_link_is_still_a_conflict() {
    let f = Fixture::new();
    let os = f.layer("os", &[("usr/lib/a", "one")], &[("lib", "usr/lib")]);
    let other = f.layer("other", &[("lib/a", "two")], &[]);
    let why = f.merged(&[os, other]).expect_err("the same file twice");
    assert!(
        why.contains("/usr/lib/a: claimed by both os and other"),
        "{why}"
    );
}

#[test]
fn a_link_that_names_no_directory_of_the_image_is_not_followed() {
    let f = Fixture::new();
    // To a path that is made when the system runs: nothing to merge into.
    let os = f.layer("os", &[("etc/x", "x")], &[("etc/board", "/run/board")]);
    let other = f.layer("other", &[("etc/board/security", "off")], &[]);
    let why = f.merged(&[os, other]).expect_err("a link and a directory");
    assert!(why.contains("etc/board"), "{why}");
    assert!(why.contains("os") && why.contains("other"), "{why}");
    // Out of the image.
    let os = f.layer("os2", &[("usr/lib/k", "k")], &[("lib", "../../lib")]);
    let other = f.layer("other2", &[("lib/a", "a")], &[]);
    assert!(f.merged(&[os, other]).is_err());
}
