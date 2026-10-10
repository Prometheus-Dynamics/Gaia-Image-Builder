//! BusyBox runtime libraries: the closure copied into a tree. Split from
//! `tests.rs` for its length.

use super::*;
use gaia_plan::{RuntimeClosure, RuntimeEntry};
use std::fs;
use std::path::{Path, PathBuf};

/// A BusyBox built for the target, in the read-only target tree of a
/// Buildroot build. The test that reads it is skipped when it is absent.
const TARGET_BUSYBOX: &str = "/dev/shm/gaia-sozo/217f87a2dbce/buildroot-output/target/bin/busybox";

fn write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent dir");
    }
    fs::write(path, bytes).expect("fixture file");
}

#[cfg(unix)]
#[test]
fn copies_runtime_closure_files_and_symlink_chain_into_the_tree() {
    let root = unique_dir("gaia-busybox-closure-copy");
    let sysroot = root.join("target");
    let tree = root.join("tree");
    write(&sysroot.join("lib/ld-test-real.so"), b"loader bytes");
    write(&sysroot.join("lib/libc-2.0.so"), b"libc bytes");
    let closure = RuntimeClosure {
        sysroot: Some(sysroot.clone()),
        dynamic: true,
        interpreter: Some("/lib/ld-test.so.1".into()),
        entries: vec![
            RuntimeEntry::File {
                guest: "/lib/ld-test-real.so".into(),
                source: sysroot.join("lib/ld-test-real.so"),
            },
            RuntimeEntry::Symlink {
                guest: "/lib/ld-test.so.1".into(),
                target: "ld-test-real.so".into(),
            },
            RuntimeEntry::File {
                guest: "/lib/libc-2.0.so".into(),
                source: sysroot.join("lib/libc-2.0.so"),
            },
            RuntimeEntry::Symlink {
                guest: "/lib/libc.so.6".into(),
                target: "libc-2.0.so".into(),
            },
        ],
    };

    let copied = copy_busybox_runtime_closure(&tree, &closure).expect("copy");

    assert_eq!(
        copied,
        vec![
            tree.join("lib/ld-test-real.so"),
            tree.join("lib/libc-2.0.so")
        ]
    );
    assert_eq!(
        fs::read(tree.join("lib/ld-test.so.1")).expect("interpreter through link"),
        b"loader bytes"
    );
    assert_eq!(
        fs::read(tree.join("lib/libc.so.6")).expect("soname through link"),
        b"libc bytes"
    );
    assert_eq!(
        fs::read_link(tree.join("lib/libc.so.6")).expect("link"),
        PathBuf::from("libc-2.0.so")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn copy_replaces_a_stale_entry_but_refuses_a_directory() {
    let root = unique_dir("gaia-busybox-closure-replace");
    let sysroot = root.join("target");
    let tree = root.join("tree");
    write(&sysroot.join("lib/libx.so.1"), b"new");
    write(&tree.join("lib/libx.so.1"), b"stale");
    let file_closure = RuntimeClosure {
        sysroot: Some(sysroot.clone()),
        dynamic: true,
        interpreter: None,
        entries: vec![RuntimeEntry::File {
            guest: "/lib/libx.so.1".into(),
            source: sysroot.join("lib/libx.so.1"),
        }],
    };
    copy_busybox_runtime_closure(&tree, &file_closure).expect("replace stale file");
    assert_eq!(fs::read(tree.join("lib/libx.so.1")).expect("file"), b"new");

    fs::create_dir_all(tree.join("lib/dir.so")).expect("dir in the way");
    let dir_closure = RuntimeClosure {
        entries: vec![RuntimeEntry::File {
            guest: "/lib/dir.so".into(),
            source: sysroot.join("lib/libx.so.1"),
        }],
        ..file_closure
    };
    let error = copy_busybox_runtime_closure(&tree, &dir_closure).expect_err("directory");
    assert!(error.contains("is a directory"), "{error}");
    let _ = fs::remove_dir_all(root);
}

/// The target BusyBox's loader and C library come from the target sysroot:
/// the closure names them at their absolute paths, and each file it copies
/// is a file under the sysroot. Skipped when the target tree is not present.
#[cfg(unix)]
#[test]
fn target_busybox_resolves_loader_and_libc_from_its_target_sysroot() {
    let busybox = Path::new(TARGET_BUSYBOX);
    if !busybox.is_file() {
        eprintln!("skipped: {TARGET_BUSYBOX} is not present");
        return;
    }
    let sysroot = busybox
        .parent()
        .and_then(Path::parent)
        .expect("target sysroot");
    let closure = gaia_plan::resolve_runtime_closure(busybox, None).expect("closure");

    assert!(closure.dynamic);
    assert_eq!(
        closure.interpreter.as_deref(),
        Some("/lib/ld-linux-aarch64.so.1")
    );
    assert_eq!(closure.sysroot.as_deref(), Some(sysroot));
    // The target root links /lib to usr/lib, so the loader and libc are
    // found through that link and their real files sit under /usr/lib.
    let guests: Vec<&str> = closure.entries.iter().map(RuntimeEntry::guest).collect();
    assert!(
        guests.contains(&"/usr/lib/ld-linux-aarch64.so.1"),
        "{guests:?}"
    );
    assert!(guests.contains(&"/usr/lib/libc.so.6"), "{guests:?}");
    assert!(guests.contains(&"/lib"), "{guests:?}");
    for entry in &closure.entries {
        if let RuntimeEntry::File { source, .. } = entry {
            assert!(source.starts_with(sysroot), "{}", source.display());
        }
    }

    let tree = unique_dir("gaia-busybox-target-closure");
    let copied = copy_busybox_runtime_closure(&tree, &closure).expect("copy target closure");
    // Looked up by the names the loader uses, through the copied /lib link.
    let loader = fs::read(tree.join("lib/ld-linux-aarch64.so.1")).expect("loader in the tree");
    let libc = fs::read(tree.join("lib/libc.so.6")).expect("libc.so.6 in the tree");
    assert_eq!(&loader[..4], b"\x7fELF", "loader is an ELF object");
    assert_eq!(&libc[..4], b"\x7fELF", "libc.so.6 is an ELF object");
    assert!(
        copied
            .iter()
            .any(|path| path.starts_with(tree.join("usr/lib"))),
        "{copied:?}"
    );
    let _ = fs::remove_dir_all(tree);
}
