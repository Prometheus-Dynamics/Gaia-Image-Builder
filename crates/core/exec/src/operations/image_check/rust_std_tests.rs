//! Copies of the Rust standard library per program, in a root filesystem of
//! generated ELF files.

use super::rust_std::{Duplicated, copies};
use gaia_plan::test_fixtures::{EM_X86_64, ElfFixture, build_elf};
use gaia_testkit::TempDir;
use std::path::Path;

const INTERPRETER: &str = "/lib64/ld-linux-x86-64.so.2";

/// An object needing `needed`, a program when `program`, holding std's
/// panic locations when `std`.
fn put(root: &Path, guest: &str, program: bool, needed: &[&str], std: bool) {
    let mut fixture = ElfFixture::dynamic(EM_X86_64, needed);
    fixture.interpreter = program.then_some(INTERPRETER);
    let mut bytes = build_elf(&fixture);
    if std {
        bytes.extend_from_slice(b"/rustc/0123/library/std/src/io/mod.rs\0");
    }
    let path = root.join(guest.trim_start_matches('/'));
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    std::fs::write(path, bytes).expect("write");
}

#[test]
fn a_program_and_a_rust_library_it_loads_each_carry_std() {
    let scratch = TempDir::new("gaia-image-check-rust-std");
    let root = scratch.path();
    put(root, "/usr/lib/libcapi.so", false, &["libc.so.6"], true);
    put(root, "/usr/lib/libc.so.6", false, &[], false);
    put(root, "/usr/lib/libstd-abc.so", false, &["libc.so.6"], true);
    // Statically linked Rust, and a C ABI library built from Rust.
    put(
        root,
        "/usr/bin/twice",
        true,
        &["libcapi.so", "libc.so.6"],
        true,
    );
    // Linked with prefer-dynamic: its std strings are generic code, not std.
    put(
        root,
        "/usr/bin/shared",
        true,
        &["libstd-abc.so", "libc.so.6"],
        true,
    );
    // C on the library: the library's copy only.
    put(
        root,
        "/usr/bin/c-client",
        true,
        &["libcapi.so", "libc.so.6"],
        false,
    );
    put(root, "/usr/bin/plain", true, &["libc.so.6"], false);
    // A static program: no interpreter, `ET_EXEC`, its own copy.
    put(root, "/usr/bin/static", false, &[], true);
    let path = root.join("usr/bin/static");
    let mut bytes = std::fs::read(&path).expect("read");
    bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
    std::fs::write(&path, bytes).expect("write");

    let found = copies(root, &Default::default());

    assert_eq!(found.rust_programs, 4);
    assert_eq!(
        found.duplicated,
        vec![Duplicated {
            program: "/usr/bin/twice".into(),
            copies: vec!["/usr/bin/twice".into(), "/usr/lib/libcapi.so".into()],
        }]
    );
    let summary = found.summary().expect("a duplicate is summarized");
    assert!(
        summary.starts_with("1 program(s) run with more than one copy"),
        "{summary}"
    );
    assert_eq!(found.json()["duplicated"][0]["program"], "/usr/bin/twice");
}

#[test]
fn without_rust_there_is_nothing_to_report() {
    let scratch = TempDir::new("gaia-image-check-rust-std-none");
    put(scratch.path(), "/usr/bin/plain", true, &[], false);
    let found = copies(scratch.path(), &Default::default());
    assert_eq!(found.rust_programs, 0);
    assert!(found.summary().is_none());
}

/// A real root filesystem, `GAIA_STD_ROOTFS`. Prints what the report finds.
#[test]
#[ignore = "reads a real root filesystem named by the environment"]
fn a_real_root_filesystem() {
    let root = std::env::var("GAIA_STD_ROOTFS").expect("GAIA_STD_ROOTFS");
    let started = std::time::Instant::now();
    let found = copies(Path::new(&root), &Default::default());
    println!(
        "{} Rust program(s) in {:?}",
        found.rust_programs,
        started.elapsed()
    );
    for process in &found.duplicated {
        println!("{} {:?}", process.program, process.copies);
    }
}

/// A plugin the surface opens runs in each program that loads the surface.
#[test]
fn a_surface_plugin_counts_in_the_programs_that_load_the_surface() {
    let scratch = TempDir::new("gaia-image-check-rust-std-plugins");
    let root = scratch.path();
    put(root, "/usr/lib/libsurface.so.1", false, &[], false);
    put(root, "/usr/lib/plugins/eidos.so", false, &[], true);
    put(root, "/usr/bin/host", true, &["libsurface.so.1"], true);
    put(root, "/usr/bin/other", true, &[], true);
    let plugins = [(
        "libsurface.so.1".to_string(),
        vec!["/usr/lib/plugins/eidos.so".to_string()],
    )]
    .into_iter()
    .collect();

    let found = copies(root, &plugins);

    assert_eq!(found.rust_programs, 2);
    assert_eq!(
        found.duplicated,
        vec![Duplicated {
            program: "/usr/bin/host".into(),
            copies: vec!["/usr/bin/host".into(), "/usr/lib/plugins/eidos.so".into()],
        }]
    );
}
