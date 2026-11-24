//! `gaia.source.input`: a file as a one-file tree under its name, keyed on
//! its content and name, not its path.

use gaia_source_files::source_input;
use gaia_step::{Ctx, logs_dir, step_records};
use gaia_store::{Digest, Store};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn store_str(root: &Path) -> String {
    root.join("store").display().to_string()
}

fn write(path: &Path, bytes: &[u8]) -> PathBuf {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, bytes).expect("write");
    path.to_path_buf()
}

/// The tree's digest, and the directory it is stored in.
fn stored(root: &Path, path: &Path, name: &str) -> (Digest, PathBuf) {
    let tree = source_input(
        &store_str(root),
        &path.display().to_string(),
        name,
        &Ctx::new(),
    )
    .expect("stores");
    let object = tree.object.object_ref().expect("object");
    let store = Store::open(root.join("store")).expect("store");
    (object.digest, store.path(&object))
}

#[test]
fn the_file_is_one_file_in_the_tree_under_its_name() {
    let root = tempfile::tempdir().expect("tempdir");
    let key = write(
        &root.path().join("keys/bench.pub"),
        b"ssh-ed25519 AAAA test\n",
    );
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    let (_, dir) = stored(root.path(), &key, "bench-key");
    let names: Vec<String> = std::fs::read_dir(&dir)
        .expect("tree")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["bench-key"]);
    let copy = dir.join("bench-key");
    assert_eq!(
        std::fs::read(&copy).expect("bytes"),
        b"ssh-ed25519 AAAA test\n"
    );
    // Stored as 0644 whatever the file's own mode.
    let mode = std::fs::metadata(&copy).expect("meta").permissions().mode() & 0o7777;
    assert_eq!(mode, 0o644);
}

#[test]
fn the_same_bytes_from_two_paths_are_the_same_tree() {
    let root = tempfile::tempdir().expect("tempdir");
    let a = write(&root.path().join("one/key.pub"), b"same\n");
    let b = write(&root.path().join("two/elsewhere.txt"), b"same\n");
    let (first, _) = stored(root.path(), &a, "bench-key");
    let (second, _) = stored(root.path(), &b, "bench-key");
    assert_eq!(first, second);
    // The second call is the same step as the first: found, not made.
    let store = Store::open(root.path().join("store")).expect("store");
    let made: Vec<bool> = step_records(&logs_dir(&store))
        .iter()
        .map(|r| r.made)
        .collect();
    assert_eq!(made, [true, false]);
}

#[test]
fn changed_bytes_are_another_tree_and_another_name_is_too() {
    let root = tempfile::tempdir().expect("tempdir");
    let key = write(&root.path().join("key.pub"), b"one\n");
    let (first, _) = stored(root.path(), &key, "bench-key");
    write(&key, b"two\n");
    let (second, _) = stored(root.path(), &key, "bench-key");
    assert_ne!(first, second);
    let (renamed, _) = stored(root.path(), &key, "other-key");
    assert_ne!(second, renamed);
}

#[test]
fn a_directory_or_a_missing_path_fails() {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = root.path().join("keys");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let error = source_input(
        &store_str(root.path()),
        &dir.display().to_string(),
        "bench-key",
        &Ctx::new(),
    )
    .expect_err("a directory is refused");
    assert!(
        error.to_string().contains("is not a regular file"),
        "{error}"
    );
    let missing = root.path().join("keys/none.pub");
    let error = source_input(
        &store_str(root.path()),
        &missing.display().to_string(),
        "bench-key",
        &Ctx::new(),
    )
    .expect_err("a missing path is refused");
    assert!(
        error.to_string().contains("is not a regular file"),
        "{error}"
    );
}

#[test]
fn a_name_that_is_not_one_file_name_fails() {
    let root = tempfile::tempdir().expect("tempdir");
    let key = write(&root.path().join("key.pub"), b"k\n");
    for name in ["", ".", "..", "a/b"] {
        let error = source_input(
            &store_str(root.path()),
            &key.display().to_string(),
            name,
            &Ctx::new(),
        )
        .expect_err("refused");
        assert!(error.to_string().contains("is not a file name"), "{error}");
    }
}
