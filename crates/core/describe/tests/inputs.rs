//! A named input, `{ input = NAME }`: read in a component's install entry,
//! refused in a board's file references, and its name checked at its key.

use gaia_describe::{Board, Component, Error, FileRef, InstallFrom};
use std::path::{Path, PathBuf};

/// Writes `files` (relative path, text) under a new directory.
fn files(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for (path, text) in files {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }
    dir
}

/// A component file with one install entry `from`, read.
fn install_from(from: &str) -> (Result<Component, Error>, tempfile::TempDir) {
    let dir = files(&[
        (
            "component.toml",
            &format!(
                "version = 1\nname = \"c\"\n[[install]]\nfrom = {from}\nto = \"/root/.ssh/authorized_keys\"\nmode = \"0600\"\n"
            ),
        ),
        ("files/a", "a\n"),
    ]);
    let read = Component::read(dir.path().join("component.toml"));
    (read, dir)
}

fn refused(from: &str) -> (Error, tempfile::TempDir) {
    let (read, dir) = install_from(from);
    let error = read.expect_err("an error");
    println!("{error}");
    (error, dir)
}

fn assert_names(error: &Error, file: &Path, key: &str, says: &str) {
    assert_eq!(error.file, file, "{error}");
    assert_eq!(error.key, key, "{error}");
    assert!(error.message.contains(says), "{error}");
}

#[test]
fn an_install_entry_reads_a_named_input() {
    let (read, _dir) = install_from("{ input = \"bench-key\" }");
    let component = read.expect("reads");
    assert_eq!(
        component.install[0].from,
        InstallFrom::File(FileRef::Input {
            name: "bench-key".into()
        })
    );
    assert_eq!(component.install[0].to, "/root/.ssh/authorized_keys");
}

#[test]
fn the_existing_forms_read_as_they_did() {
    let (read, _dir) = install_from("{ file = \"files/a\" }");
    let component = read.expect("reads");
    assert!(matches!(
        &component.install[0].from,
        InstallFrom::File(FileRef::Local { .. })
    ));
    let (read, _dir) = install_from("{ source = \"s\", path = \"p\" }");
    let error = read.expect_err("no source named s");
    assert!(error.message.contains("no source named `s`"), "{error}");
    assert_eq!(error.key, "install[0].from.source");
}

#[test]
fn a_bad_input_name_is_an_error_at_its_key() {
    for name in ["Bench", "bench_key", "", "bench key", "bench/key"] {
        let (error, dir) = refused(&format!("{{ input = \"{name}\" }}"));
        assert_names(
            &error,
            &dir.path().join("component.toml"),
            "install[0].from.input",
            "lowercase letters, digits and `-`",
        );
    }
}

#[test]
fn an_input_combined_with_a_file_is_an_error_at_from() {
    for from in [
        "{ input = \"bench-key\", file = \"files/a\" }",
        "{ input = \"bench-key\", source = \"s\", path = \"p\" }",
        "{ input = \"bench-key\", path = \"p\" }",
    ] {
        let (error, dir) = refused(from);
        assert_names(
            &error,
            &dir.path().join("component.toml"),
            "install[0].from",
            "not combined with `source`, `path` or `file`",
        );
    }
}

#[test]
fn an_input_in_a_board_file_is_an_error_at_its_key() {
    let dir = files(&[(
        "b/board.toml",
        "version = 1\nname = \"b\"\n[target]\narch = \"aarch64\"\n[[firmware]]\nfrom = { input = \"bench-key\" }\nto = \"/lib/firmware/x\"\n",
    )]);
    let path: PathBuf = dir.path().join("b/board.toml");
    let error = Board::read(&path).expect_err("an error");
    println!("{error}");
    assert_names(
        &error,
        &path,
        "firmware[0].from",
        "a named input is only for a component's install entry",
    );
}
