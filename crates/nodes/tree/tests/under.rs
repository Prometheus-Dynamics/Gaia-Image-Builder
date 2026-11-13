//! `gaia.tree.under`: one step's tree as a layer under a directory.

use gaia_step::Ctx;
use gaia_store::Store;
use gaia_tree::tree_under;
use gaia_types::{Artifact, ArtifactKind};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// The directory `make` fills, stored as a tree artifact.
fn stored(root: &Path, store: &str, make: impl Fn(&Path)) -> Artifact {
    let dir = root.join("src");
    std::fs::create_dir_all(&dir).expect("mkdir");
    make(&dir);
    let store = Store::open(store).expect("store");
    Artifact {
        kind: ArtifactKind::parse("modules"),
        object: store.put_tree(&dir).expect("put").into(),
    }
}

#[test]
fn a_tree_goes_under_the_prefix_with_its_modes_and_links() {
    let root = tempfile::tempdir().expect("tempdir");
    let store = root.path().join("store").display().to_string();
    let from = stored(root.path(), &store, |dir| {
        std::fs::create_dir_all(dir.join("6.1/kernel")).expect("mkdir");
        std::fs::write(dir.join("6.1/kernel/a.ko"), "module").expect("write");
        std::fs::write(dir.join("6.1/run"), "#!/bin/sh\n").expect("write");
        std::fs::set_permissions(dir.join("6.1/run"), std::fs::Permissions::from_mode(0o755))
            .expect("chmod");
        std::os::unix::fs::symlink("kernel/a.ko", dir.join("6.1/first")).expect("link");
    });
    let layer = tree_under(&store, "demo.kernel", &from, "/lib/modules", &Ctx::new())
        .expect("under");
    assert_eq!(layer.owner, "demo.kernel");
    let opened = Store::open(&store).expect("store");
    let out = layer.tree.object.path_in(&opened).expect("in the store");
    let base = out.join("lib/modules/6.1");
    assert_eq!(
        std::fs::read_to_string(base.join("kernel/a.ko")).expect("read"),
        "module"
    );
    let mode = std::fs::metadata(base.join("run")).expect("stat").permissions().mode();
    assert_eq!(mode & 0o777, 0o755, "the file's own mode");
    assert_eq!(
        std::fs::read_link(base.join("first")).expect("a link stays a link"),
        Path::new("kernel/a.ko")
    );
    // Nothing beside the prefix.
    let top: Vec<_> = std::fs::read_dir(&out)
        .expect("read")
        .map(|e| e.expect("entry").file_name())
        .collect();
    assert_eq!(top, ["lib"]);
}

#[test]
fn an_empty_tree_gives_an_empty_layer_without_the_prefix() {
    let root = tempfile::tempdir().expect("tempdir");
    let store = root.path().join("store").display().to_string();
    let from = stored(root.path(), &store, |_| {});
    let layer =
        tree_under(&store, "demo.kernel", &from, "/lib/modules", &Ctx::new()).expect("under");
    let opened = Store::open(&store).expect("store");
    let out = layer.tree.object.path_in(&opened).expect("in the store");
    assert_eq!(std::fs::read_dir(&out).expect("read").count(), 0);
}

#[test]
fn a_file_and_a_prefix_outside_the_image_are_refused() {
    let root = tempfile::tempdir().expect("tempdir");
    let store = root.path().join("store").display().to_string();
    let from = stored(root.path(), &store, |dir| {
        std::fs::write(dir.join("Image"), "kernel").expect("write");
    });
    let err = tree_under(&store, "demo.kernel", &from, "/boot/../..", &Ctx::new())
        .expect_err("a prefix that leaves the image");
    println!("{err}");
    let err = tree_under(&store, "", &from, "/boot", &Ctx::new()).expect_err("no owner");
    assert!(err.to_string().contains("a layer needs an owner"), "{err}");

    let file = root.path().join("one");
    std::fs::write(&file, "kernel").expect("write");
    let opened = Store::open(&store).expect("store");
    let one = Artifact {
        kind: ArtifactKind::parse("kernel"),
        object: opened.put_file(&file).expect("put").into(),
    };
    let err = tree_under(&store, "demo.kernel", &one, "/boot", &Ctx::new())
        .expect_err("a file is not a tree");
    assert!(
        err.to_string().contains("is a file, not a tree of files"),
        "{err}"
    );
}
