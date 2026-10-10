//! Tests for the sysroot-confined runtime closure. Each test builds a small
//! target root in a temporary directory from generated ELF objects.

use super::*;
use crate::elf::tests::{EM_AARCH64, EM_X86_64, ElfFixture, build_elf};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_root(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "gaia-runtime-libs-{name}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("temp root");
    dir
}

fn write_file(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent dir");
    }
    fs::write(path, bytes).expect("fixture file");
}

#[cfg(unix)]
fn symlink(target: &str, path: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent dir");
    }
    std::os::unix::fs::symlink(target, path).expect("fixture symlink");
}

/// One line per entry, sorted by target path: `file <guest>` or
/// `link <guest> -> <target>`.
fn summary(closure: &RuntimeClosure) -> Vec<String> {
    closure
        .entries
        .iter()
        .map(|entry| match entry {
            RuntimeEntry::File { guest, .. } => format!("file {guest}"),
            RuntimeEntry::Symlink { guest, target } => format!("link {guest} -> {target}"),
        })
        .collect()
}

#[cfg(unix)]
#[test]
fn resolves_interpreter_needed_chain_symlinks_and_runpath_inside_the_sysroot() {
    let root = temp_root("chain");
    let sysroot = root.join("target");
    write_file(
        &sysroot.join("bin/busybox"),
        &build_elf(&ElfFixture {
            interpreter: Some("/lib/ld-test.so.1"),
            ..ElfFixture::dynamic(EM_AARCH64, &["libc.so.6", "libz.so.1"])
        }),
    );
    write_file(
        &sysroot.join("lib/ld-test-real.so"),
        &build_elf(&ElfFixture::dynamic(EM_AARCH64, &[])),
    );
    symlink("ld-test-real.so", &sysroot.join("lib/ld-test.so.1"));
    write_file(
        &sysroot.join("lib/libc-2.0.so"),
        &build_elf(&ElfFixture {
            runpath: Some("$ORIGIN/../usr/lib"),
            ..ElfFixture::dynamic(EM_AARCH64, &["libm.so.6"])
        }),
    );
    symlink("libc-2.0.so", &sysroot.join("lib/libc.so.6"));
    write_file(
        &sysroot.join("usr/lib/libm.so.6"),
        &build_elf(&ElfFixture::dynamic(EM_AARCH64, &[])),
    );
    write_file(
        &sysroot.join("usr/lib/libz.so.1.2"),
        &build_elf(&ElfFixture::dynamic(EM_AARCH64, &[])),
    );
    symlink("/usr/lib/libz.so.1.2", &sysroot.join("lib/libz.so.1"));

    let closure = resolve_runtime_closure(&sysroot.join("bin/busybox"), None).expect("closure");

    assert!(closure.dynamic);
    assert_eq!(closure.interpreter.as_deref(), Some("/lib/ld-test.so.1"));
    assert_eq!(closure.sysroot.as_deref(), Some(sysroot.as_path()));
    assert_eq!(
        summary(&closure),
        vec![
            "file /lib/ld-test-real.so",
            "link /lib/ld-test.so.1 -> ld-test-real.so",
            "file /lib/libc-2.0.so",
            "link /lib/libc.so.6 -> libc-2.0.so",
            "link /lib/libz.so.1 -> /usr/lib/libz.so.1.2",
            "file /usr/lib/libm.so.6",
            "file /usr/lib/libz.so.1.2",
        ]
    );
    let libm = closure
        .entries
        .iter()
        .find(|entry| entry.guest() == "/usr/lib/libm.so.6")
        .expect("libm entry");
    assert!(matches!(
        libm,
        RuntimeEntry::File { source, .. } if *source == sysroot.join("usr/lib/libm.so.6")
    ));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn static_binary_has_no_closure_and_needs_no_sysroot() {
    let root = temp_root("static");
    let busybox = root.join("static-busybox");
    write_file(&busybox, &build_elf(&ElfFixture::dynamic(EM_AARCH64, &[])));

    let closure = resolve_runtime_closure(&busybox, None).expect("static closure");

    assert!(!closure.dynamic);
    assert!(closure.entries.is_empty());
    assert!(closure.interpreter.is_none());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn missing_libraries_are_all_named_in_one_error() {
    let root = temp_root("missing");
    let sysroot = root.join("target");
    write_file(
        &sysroot.join("bin/busybox"),
        &build_elf(&ElfFixture::dynamic(
            EM_AARCH64,
            &["libgone.so.1", "libgone-two.so"],
        )),
    );

    let error =
        resolve_runtime_closure(&sysroot.join("bin/busybox"), None).expect_err("missing libraries");

    assert!(error.contains("libgone.so.1 (needed by"), "{error}");
    assert!(error.contains("libgone-two.so (needed by"), "{error}");
    assert!(error.contains(&sysroot.display().to_string()), "{error}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn sysroot_is_derived_from_the_binary_location_or_must_be_given() {
    assert_eq!(
        derive_sysroot(Path::new("/a/b/bin/busybox")),
        Some(PathBuf::from("/a/b"))
    );
    assert_eq!(
        derive_sysroot(Path::new("/a/b/sbin/busybox")),
        Some(PathBuf::from("/a/b"))
    );
    assert_eq!(
        derive_sysroot(Path::new("/a/usr/bin/busybox")),
        Some(PathBuf::from("/a"))
    );
    assert_eq!(
        derive_sysroot(Path::new("/a/usr/sbin/busybox")),
        Some(PathBuf::from("/a"))
    );
    assert_eq!(derive_sysroot(Path::new("/a/busybox")), None);
    assert_eq!(derive_sysroot(Path::new("/bin/busybox")), None);

    let root = temp_root("explicit");
    let sysroot = root.join("target");
    let busybox = root.join("busybox");
    write_file(
        &busybox,
        &build_elf(&ElfFixture {
            interpreter: Some("/lib/ld-explicit.so"),
            ..ElfFixture::dynamic(EM_AARCH64, &[])
        }),
    );
    write_file(
        &sysroot.join("lib/ld-explicit.so"),
        &build_elf(&ElfFixture::dynamic(EM_AARCH64, &[])),
    );
    let error = resolve_runtime_closure(&busybox, None).expect_err("no sysroot");
    assert!(error.contains("set `sysroot`"), "{error}");

    let closure = resolve_runtime_closure(&busybox, Some(&sysroot)).expect("explicit sysroot");
    assert_eq!(summary(&closure), vec!["file /lib/ld-explicit.so"]);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn library_for_another_architecture_is_not_used() {
    let root = temp_root("arch");
    let sysroot = root.join("target");
    write_file(
        &sysroot.join("bin/busybox"),
        &build_elf(&ElfFixture::dynamic(EM_AARCH64, &["libq.so.1"])),
    );
    write_file(
        &sysroot.join("lib/libq.so.1"),
        &build_elf(&ElfFixture::dynamic(EM_X86_64, &[])),
    );
    write_file(
        &sysroot.join("usr/lib/libq.so.1"),
        &build_elf(&ElfFixture::dynamic(EM_AARCH64, &[])),
    );

    let closure = resolve_runtime_closure(&sysroot.join("bin/busybox"), None).expect("closure");

    assert_eq!(summary(&closure), vec!["file /usr/lib/libq.so.1"]);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn ld_so_conf_directories_and_includes_are_searched() {
    let root = temp_root("ldconf");
    let sysroot = root.join("target");
    write_file(
        &sysroot.join("bin/busybox"),
        &build_elf(&ElfFixture::dynamic(EM_AARCH64, &["libext.so.1"])),
    );
    write_file(
        &sysroot.join("etc/ld.so.conf"),
        b"# comment\ninclude /etc/ld.so.conf.d/*.conf\n/opt/extra # trailing\n",
    );
    write_file(&sysroot.join("etc/ld.so.conf.d/a.conf"), b"/opt/a\n");
    write_file(&sysroot.join("etc/ld.so.conf.d/b.conf"), b"/opt/b\n");
    write_file(&sysroot.join("etc/ld.so.conf.d/notes.txt"), b"/opt/never\n");
    write_file(
        &sysroot.join("opt/b/libext.so.1"),
        &build_elf(&ElfFixture::dynamic(EM_AARCH64, &[])),
    );

    let closure = resolve_runtime_closure(&sysroot.join("bin/busybox"), None).expect("closure");

    assert_eq!(summary(&closure), vec!["file /opt/b/libext.so.1"]);
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn symlink_loop_is_an_error_not_a_hang() {
    let root = temp_root("loop");
    let sysroot = root.join("target");
    write_file(
        &sysroot.join("bin/busybox"),
        &build_elf(&ElfFixture::dynamic(EM_AARCH64, &["loopy.so.1"])),
    );
    symlink("loopy.so.1", &sysroot.join("lib/loopy.so.1"));

    let error =
        resolve_runtime_closure(&sysroot.join("bin/busybox"), None).expect_err("symlink loop");

    assert!(error.contains("too many symbolic links"), "{error}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn wildcard_matches_star_and_question_mark_within_one_name() {
    assert!(wildcard_matches(b"*.conf", b"a.conf"));
    assert!(!wildcard_matches(b"*.conf", b"a.txt"));
    assert!(wildcard_matches(b"lib?.so", b"libc.so"));
    assert!(!wildcard_matches(b"lib?.so", b"lib.so"));
    assert!(wildcard_matches(b"*", b""));
}

/// A merged-usr target (`/lib -> usr/lib`, `/lib64 -> lib`) whose loader
/// searches only /lib64 and /usr/lib64 (aarch64 glibc): the library-directory
/// links come along, so the loader finds what the lookup found under /usr/lib.
#[cfg(unix)]
#[test]
fn library_directory_links_of_the_sysroot_are_kept() {
    let root = temp_root("dirlinks");
    let sysroot = root.join("target");
    write_file(
        &sysroot.join("bin/busybox"),
        &build_elf(&ElfFixture {
            interpreter: Some("/lib/ld-linux-aarch64.so.1"),
            ..ElfFixture::dynamic(EM_AARCH64, &["libresolv.so.2"])
        }),
    );
    write_file(
        &sysroot.join("usr/lib/ld-linux-aarch64.so.1"),
        &build_elf(&ElfFixture::dynamic(EM_AARCH64, &[])),
    );
    write_file(
        &sysroot.join("usr/lib/libresolv.so.2"),
        &build_elf(&ElfFixture::dynamic(EM_AARCH64, &[])),
    );
    symlink("usr/lib", &sysroot.join("lib"));
    symlink("lib", &sysroot.join("lib64"));

    let closure = resolve_runtime_closure(&sysroot.join("bin/busybox"), None).expect("closure");

    assert_eq!(
        summary(&closure),
        vec![
            "link /lib -> usr/lib",
            "link /lib64 -> lib",
            "file /usr/lib/ld-linux-aarch64.so.1",
            "file /usr/lib/libresolv.so.2",
        ]
    );
    let _ = fs::remove_dir_all(root);
}
