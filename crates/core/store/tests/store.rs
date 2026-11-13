use gaia_store::{Check, Digest, Keep, Kind, ObjectRef, StepKey, Store, file_digest, tree_digest};
use std::fs::{self, File, FileTimes};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::time::{Duration, SystemTime};

fn chmod(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod");
}

fn set_mtime(path: &Path, seconds: u64) {
    let time = SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
    File::options()
        .write(true)
        .open(path)
        .expect("open")
        .set_times(FileTimes::new().set_modified(time))
        .expect("mtime");
}

/// The same tree, written in the given order of its files, with mtimes
/// derived from `stamp`.
fn sample_tree(root: &Path, reverse: bool, stamp: u64) {
    fs::create_dir_all(root.join("usr/bin")).expect("dirs");
    fs::create_dir_all(root.join("etc/empty")).expect("dirs");
    let mut files = vec![
        ("usr/bin/tool", &b"#!/bin/sh\necho hi\n"[..], 0o755),
        ("etc/config", &b"key = value\n"[..], 0o644),
        ("etc/secret", &b"s\n"[..], 0o600),
    ];
    if reverse {
        files.reverse();
    }
    for (path, bytes, mode) in files {
        fs::write(root.join(path), bytes).expect("write");
        chmod(&root.join(path), mode);
        set_mtime(&root.join(path), stamp);
    }
    symlink("../usr/bin/tool", root.join("etc/tool")).expect("link");
    chmod(&root.join("usr"), 0o755);
    chmod(&root.join("usr/bin"), 0o755);
    chmod(&root.join("etc"), 0o755);
    chmod(&root.join("etc/empty"), 0o700);
}

#[test]
fn a_file_digest_is_its_sha256() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("a");
    fs::write(&path, b"a").expect("write");
    assert_eq!(
        file_digest(&path).expect("digest").to_string(),
        "sha256:ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb"
    );
    let digest = Digest::of_bytes(b"a");
    assert_eq!(Digest::parse(&digest.to_string()), Some(digest));
    assert_eq!(Digest::parse(&digest.hex()), Some(digest));
    assert_eq!(Digest::parse("sha256:zz"), None);
}

#[test]
fn the_same_tree_has_the_same_digest_in_two_layouts() {
    // Two "machines": different directories, write order, mtimes and root
    // modes, and two stores at different paths.
    let one = tempfile::tempdir().expect("tempdir");
    let two = tempfile::tempdir().expect("tempdir");
    let a = one.path().join("build/out/rootfs");
    let b = two.path().join("elsewhere/x");
    sample_tree(&a, false, 1_000);
    sample_tree(&b, true, 2_000_000_000);
    chmod(&a, 0o700);
    chmod(&b, 0o755);
    let digest = tree_digest(&a).expect("digest");
    assert_eq!(tree_digest(&b).expect("digest"), digest);

    let store_one = Store::open(one.path().join("store")).expect("store");
    let store_two = Store::open(two.path().join("deeper/store")).expect("store");
    let in_one = store_one.put_tree(&a).expect("put");
    let in_two = store_two.put_tree(&b).expect("put");
    assert_eq!(in_one, in_two);
    assert_eq!(
        in_one,
        ObjectRef {
            kind: Kind::Tree,
            digest
        }
    );
    assert!(store_one.check(&in_one, Check::Digest));
    assert!(store_two.check(&in_two, Check::Digest));
    // Recorded so a change to the encoding shows up here.
    println!("sample tree digest: {digest}");
}

#[test]
fn names_modes_links_and_content_change_a_tree_digest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("t");
    sample_tree(&root, false, 1);
    let base = tree_digest(&root).expect("digest");
    let changed = |change: &dyn Fn(&Path), undo: &dyn Fn(&Path), what: &str| {
        change(&root);
        assert_ne!(tree_digest(&root).expect("digest"), base, "{what}");
        undo(&root);
        assert_eq!(tree_digest(&root).expect("digest"), base, "{what} undone");
    };
    changed(
        &|r| fs::write(r.join("etc/config"), b"key = other\n").expect("write"),
        &|r| fs::write(r.join("etc/config"), b"key = value\n").expect("write"),
        "content",
    );
    changed(
        &|r| chmod(&r.join("etc/config"), 0o600),
        &|r| chmod(&r.join("etc/config"), 0o644),
        "file mode",
    );
    changed(
        &|r| chmod(&r.join("etc/empty"), 0o755),
        &|r| chmod(&r.join("etc/empty"), 0o700),
        "directory mode",
    );
    changed(
        &|r| fs::rename(r.join("etc/secret"), r.join("etc/secret2")).expect("rename"),
        &|r| fs::rename(r.join("etc/secret2"), r.join("etc/secret")).expect("rename"),
        "name",
    );
    changed(
        &|r| {
            fs::remove_file(r.join("etc/tool")).expect("rm");
            symlink("/usr/bin/tool", r.join("etc/tool")).expect("link");
        },
        &|r| {
            fs::remove_file(r.join("etc/tool")).expect("rm");
            symlink("../usr/bin/tool", r.join("etc/tool")).expect("link");
        },
        "link target",
    );
    changed(
        &|r| fs::create_dir(r.join("new")).expect("mkdir"),
        &|r| fs::remove_dir(r.join("new")).expect("rmdir"),
        "empty directory",
    );
    set_mtime(&root.join("etc/config"), 99);
    assert_eq!(tree_digest(&root).expect("digest"), base, "mtime");
}

#[test]
fn a_committed_step_is_found_and_a_missing_or_changed_output_is_not() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).expect("store");
    let key = StepKey(Digest::of_bytes(b"step"));
    assert!(
        store
            .lookup(&key, Check::Present)
            .expect("lookup")
            .is_none()
    );

    let scratch = store.scratch().expect("scratch");
    fs::write(scratch.path().join("bin"), b"binary").expect("write");
    fs::create_dir_all(scratch.path().join("tree/sub")).expect("dir");
    fs::write(scratch.path().join("tree/sub/f"), b"f").expect("write");
    let entry = scratch
        .commit_step(
            &key,
            &[("bin", Path::new("bin")), ("tree", Path::new("tree"))],
        )
        .expect("commit");
    assert_eq!(
        store.lookup(&key, Check::Digest).expect("lookup"),
        Some(entry.clone())
    );
    assert_eq!(
        fs::read_dir(dir.path().join("scratch"))
            .expect("ls")
            .count(),
        0
    );

    let tree = entry.outputs["tree"];
    fs::write(store.path(&tree).join("sub/f"), b"tampered").expect("write");
    assert!(
        store
            .lookup(&key, Check::Present)
            .expect("lookup")
            .is_some()
    );
    assert!(store.lookup(&key, Check::Digest).expect("lookup").is_none());

    fs::remove_file(store.path(&entry.outputs["bin"])).expect("rm");
    assert!(
        store
            .lookup(&key, Check::Present)
            .expect("lookup")
            .is_none()
    );
}

#[test]
fn a_dropped_scratch_leaves_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).expect("store");
    {
        let scratch = store.scratch().expect("scratch");
        fs::create_dir_all(scratch.path().join("ro/inner")).expect("dir");
        fs::write(scratch.path().join("ro/inner/f"), b"x").expect("write");
        chmod(&scratch.path().join("ro/inner"), 0o555);
        chmod(&scratch.path().join("ro"), 0o555);
        // The step failed: no commit.
    }
    for dir_name in ["scratch", "entries", "objects/file", "objects/tree"] {
        assert_eq!(
            fs::read_dir(dir.path().join(dir_name)).expect("ls").count(),
            0,
            "{dir_name}"
        );
    }
}

#[test]
fn read_only_trees_commit_and_are_removed_by_retention() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).expect("store");
    let scratch = store.scratch().expect("scratch");
    let out = scratch.path().join("cache");
    fs::create_dir_all(out.join("mod")).expect("dir");
    fs::write(out.join("mod/go.mod"), b"module x\n").expect("write");
    chmod(&out.join("mod/go.mod"), 0o444);
    chmod(&out.join("mod"), 0o555);
    chmod(&out, 0o555);
    let refs = scratch
        .commit(&[("cache", Path::new("cache"))])
        .expect("commit");
    assert!(store.check(&refs["cache"], Check::Digest));
    let report = store.retain(&Keep::default(), true).expect("retain");
    assert_eq!(report.objects_removed, 1);
    assert!(!store.check(&refs["cache"], Check::Present));
}

#[test]
fn bad_outputs_are_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).expect("store");
    for outputs in [
        vec![("up", Path::new("../x"))],
        vec![("abs", Path::new("/etc/passwd"))],
        vec![("missing", Path::new("nothing"))],
        vec![("a", Path::new("d")), ("b", Path::new("d/f"))],
    ] {
        let scratch = store.scratch().expect("scratch");
        fs::create_dir_all(scratch.path().join("d")).expect("dir");
        fs::write(scratch.path().join("d/f"), b"f").expect("write");
        assert!(scratch.commit(&outputs).is_err(), "{outputs:?}");
    }
    assert_eq!(
        fs::read_dir(dir.path().join("objects/tree"))
            .expect("ls")
            .count(),
        0
    );
}

#[test]
fn retention_keeps_what_is_referenced() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).expect("store");
    let commit = |name: &str| {
        let key = StepKey(Digest::of_bytes(name.as_bytes()));
        let scratch = store.scratch().expect("scratch");
        fs::create_dir(scratch.path().join("out")).expect("dir");
        fs::write(scratch.path().join("out/f"), name).expect("write");
        let entry = scratch
            .commit_step(&key, &[("out", Path::new("out"))])
            .expect("commit");
        (key, entry.outputs["out"])
    };
    let (kept_key, kept_object) = commit("kept by a lock");
    let (_, pinned_object) = commit("pinned object, entry dropped");
    let (dropped_key, dropped_object) = commit("dropped");
    let loose = store.put_file(&dir.path().join("objects/tree")).err();
    assert!(loose.is_some(), "a directory is not a file");

    // A dead process's half-written entry and scratch directory.
    let dead_pid = 999_999_999_u32;
    let shard = dir.path().join("entries/00");
    fs::create_dir_all(&shard).expect("dir");
    fs::write(shard.join(format!(".{dead_pid}-0-0")), b"{").expect("write");
    fs::create_dir(dir.path().join(format!("scratch/{dead_pid}-1-0"))).expect("dir");

    let mut keep = Keep::default();
    keep.entries.insert(kept_key);
    let mut other_lock = Keep::default();
    other_lock.objects.insert(pinned_object);
    keep.extend(&other_lock);

    let dry = store.retain(&keep, false).expect("dry run");
    assert_eq!((dry.entries_kept, dry.entries_removed), (1, 2));
    assert_eq!((dry.objects_kept, dry.objects_removed), (2, 1));
    assert_eq!(dry.leftovers_removed, 2);
    assert!(
        store.check(&dropped_object, Check::Present),
        "a dry run removes nothing"
    );

    assert_eq!(store.retain(&keep, true).expect("retain"), dry);
    assert!(
        store
            .lookup(&kept_key, Check::Digest)
            .expect("lookup")
            .is_some()
    );
    assert!(store.check(&kept_object, Check::Digest));
    assert!(store.check(&pinned_object, Check::Digest));
    assert!(!store.check(&dropped_object, Check::Present));
    assert!(store.entry(&dropped_key).expect("entry").is_none());
    assert_eq!(fs::read_dir(&shard).expect("ls").count(), 0);
    assert_eq!(
        fs::read_dir(dir.path().join("scratch"))
            .expect("ls")
            .count(),
        0
    );
}

/// A stored tree and its object.
fn stored(store: &Store, dir: &Path) -> ObjectRef {
    let src = dir.join("src");
    fs::create_dir_all(src.join("sub")).expect("dir");
    fs::write(src.join("sub/f"), b"content").expect("write");
    fs::write(src.join("g"), b"other").expect("write");
    store.put_tree(&src).expect("put")
}

#[test]
fn a_verified_object_is_not_read_again_until_it_is_touched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("open");
    let tree = stored(&store, dir.path());
    let record = dir
        .path()
        .join("store/verified/tree")
        .join(&tree.digest.hex()[..2])
        .join(tree.digest.hex());

    // Putting it hashed it, so its state is recorded from the start: the
    // first lookup after a build reads no content either.
    assert!(
        record.is_file(),
        "the state is recorded when it is committed"
    );
    assert!(store.check(&tree, Check::Digest));

    // An object that is already in the store is not read when the same
    // content is committed again, so that gives it no record: only a full
    // read does.
    fs::remove_file(&record).expect("remove the record");
    assert_eq!(stored(&store, dir.path()), tree);
    assert!(!record.exists(), "no record for content that was not read");
    assert!(store.check(&tree, Check::Digest));
    assert!(record.is_file(), "the full read records it");

    // Proof that the second check reads no content: the content is changed
    // with the size and the modification time put back, and the check still
    // passes. This is exactly what the record trusts.
    let file = store.path(&tree).join("sub/f");
    let before = fs::metadata(&file)
        .expect("meta")
        .modified()
        .expect("mtime");
    fs::write(&file, b"CONTENT").expect("same size, other bytes");
    File::options()
        .write(true)
        .open(&file)
        .expect("open")
        .set_times(FileTimes::new().set_modified(before))
        .expect("mtime back");
    assert!(
        store.check(&tree, Check::Digest),
        "the same state: accepted without reading"
    );

    // Touched: the state differs, so it is read in full, and fails.
    set_mtime(&file, 1_000);
    assert!(
        !store.check(&tree, Check::Digest),
        "a touched object is hashed again, and this one does not match"
    );
}

#[test]
fn a_changed_size_mode_or_entry_is_read_again() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("open");

    // A new entry.
    let tree = stored(&store, dir.path());
    assert!(store.check(&tree, Check::Digest));
    fs::write(store.path(&tree).join("sub/new"), b"").expect("write");
    assert!(!store.check(&tree, Check::Digest), "an added file");
    fs::remove_file(store.path(&tree).join("sub/new")).expect("rm");
    assert!(
        store.check(&tree, Check::Digest),
        "as it was: matches again"
    );

    // A mode.
    chmod(&store.path(&tree).join("g"), 0o600);
    assert!(!store.check(&tree, Check::Digest), "a changed mode");
}

#[test]
fn without_its_record_an_object_is_read_in_full() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path().join("store")).expect("open");
    let tree = stored(&store, dir.path());
    assert!(store.check(&tree, Check::Digest));

    // The same quiet change as above, but with the record removed: caught.
    let file = store.path(&tree).join("sub/f");
    let before = fs::metadata(&file)
        .expect("meta")
        .modified()
        .expect("mtime");
    fs::write(&file, b"CONTENT").expect("write");
    File::options()
        .write(true)
        .open(&file)
        .expect("open")
        .set_times(FileTimes::new().set_modified(before))
        .expect("mtime back");
    fs::remove_dir_all(dir.path().join("store/verified")).expect("rm");
    assert!(!store.check(&tree, Check::Digest));
}
