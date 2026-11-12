//! Each error names the file and the key.

use gaia_describe::{Board, Component, Error, Product};
use std::path::Path;

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

const FAMILY: &str = "version = 1\n[target]\narch = \"aarch64\"\n[sources.linux]\ngit = \"https://example.org/linux.git\"\nref = \"v1\"\n";

fn board_error(board: &str) -> (Error, tempfile::TempDir) {
    let dir = files(&[
        ("family.toml", FAMILY),
        ("b/board.toml", board),
        ("b/frag", ""),
    ]);
    let path = dir.path().join("b/board.toml");
    let error = Board::read(&path).expect_err("an error");
    println!("{error}");
    (error, dir)
}

fn assert_names(error: &Error, file: &Path, key: &str, says: &str) {
    assert_eq!(error.file, file, "{error}");
    assert_eq!(error.key, key, "{error}");
    assert!(error.message.contains(says), "{error}");
}

#[test]
fn an_unknown_key_names_the_key_meant() {
    let (error, dir) = board_error(
        "version = 1\nname = \"b\"\n[target]\narch = \"aarch64\"\n[kernel]\nfragmnets = [\"frag\"]\n",
    );
    assert_names(
        &error,
        &dir.path().join("b/board.toml"),
        "kernel.fragmnets",
        "did you mean `kernel.fragments`?",
    );
    assert_eq!(error.line, Some(6));

    let (error, dir) = board_error("version = 1\nnmae = \"b\"\n");
    assert_names(
        &error,
        &dir.path().join("b/board.toml"),
        "nmae",
        "did you mean `name`?",
    );
    assert_eq!(error.line, Some(2));

    let (error, dir) = board_error(
        "version = 1\nname = \"b\"\n[target]\narch = \"aarch64\"\n[sources.x]\ngti = \"u\"\nref = \"v\"\n",
    );
    assert_names(
        &error,
        &dir.path().join("b/board.toml"),
        "sources.x.gti",
        "did you mean `sources.x.git`?",
    );
}

#[test]
fn the_version_is_required_and_checked() {
    let (error, dir) = board_error("name = \"b\"\n");
    assert_names(
        &error,
        &dir.path().join("b/board.toml"),
        "version",
        "missing",
    );
    let (error, dir) = board_error("version = 3\n");
    assert_names(
        &error,
        &dir.path().join("b/board.toml"),
        "version",
        "not a version",
    );
    assert_eq!(error.line, Some(1));
}

#[test]
fn a_wrong_value_names_its_key_and_line() {
    let (error, dir) = board_error(
        "version = 1\nname = \"b\"\n[target]\narch = \"aarch64\"\n[[partitions]]\nname = \"p\"\nrole = \"raw\"\nsize = \"12Q\"\n",
    );
    assert_names(
        &error,
        &dir.path().join("b/board.toml"),
        "partitions[0].size",
        "not a size",
    );
    assert_eq!(error.line, Some(8));

    let (error, dir) = board_error("version = 1\nname = 7\n");
    assert_names(&error, &dir.path().join("b/board.toml"), "name", "string");
}

#[test]
fn sources_are_checked() {
    let (error, dir) = board_error(
        "version = 1\nname = \"b\"\n[target]\narch = \"aarch64\"\n[sources.x]\ngit = \"u\"\n",
    );
    assert_names(
        &error,
        &dir.path().join("b/board.toml"),
        "sources.x",
        "needs `ref`",
    );
    let (error, dir) = board_error(
        "version = 1\nname = \"b\"\n[target]\narch = \"aarch64\"\n[sources.x]\ngit = \"u\"\nref = \"v\"\ndownload = \"d\"\n",
    );
    assert_names(
        &error,
        &dir.path().join("b/board.toml"),
        "sources.x",
        "only one of",
    );
    // A name used that no file defines, the family's included.
    let (error, dir) = board_error(
        "version = 1\nextends = \"../family.toml\"\nname = \"b\"\n[kernel]\nsource = \"linx\"\n",
    );
    assert_names(
        &error,
        &dir.path().join("b/board.toml"),
        "kernel.source",
        "did you mean `linux`?",
    );
}

#[test]
fn paths_are_relative_and_there() {
    let (error, dir) = board_error(
        "version = 1\nname = \"b\"\n[target]\narch = \"aarch64\"\n[kernel]\nfragments = [\"frag\", \"gone\"]\n",
    );
    assert_names(
        &error,
        &dir.path().join("b/board.toml"),
        "kernel.fragments[1]",
        "is not there",
    );
    let (error, dir) = board_error(
        "version = 1\nname = \"b\"\n[target]\narch = \"aarch64\"\n[kernel]\nfragments = [\"/etc/frag\"]\n",
    );
    assert_names(
        &error,
        &dir.path().join("b/board.toml"),
        "kernel.fragments[0]",
        "absolute",
    );
}

#[test]
fn a_board_extends_once() {
    let dir = files(&[
        ("base.toml", FAMILY),
        ("family.toml", "version = 1\nextends = \"base.toml\"\n"),
        (
            "b/board.toml",
            "version = 1\nextends = \"../family.toml\"\nname = \"b\"\n",
        ),
    ]);
    let error = Board::read(dir.path().join("b/board.toml")).expect_err("an error");
    println!("{error}");
    assert_names(
        &error,
        &dir.path().join("family.toml"),
        "extends",
        "extends nothing",
    );
}

#[test]
fn required_keys_are_named() {
    let (error, dir) = board_error("version = 1\nextends = \"../family.toml\"\n");
    assert_names(&error, &dir.path().join("b/board.toml"), "name", "missing");
    let (error, dir) = board_error("version = 1\nname = \"b\"\n");
    assert_names(
        &error,
        &dir.path().join("b/board.toml"),
        "target.arch",
        "missing",
    );
}

#[test]
fn a_family_error_names_the_family_file() {
    let dir = files(&[
        ("family.toml", "version = 1\n[kernel]\nsource = \"nope\"\n"),
        (
            "b/board.toml",
            "version = 1\nextends = \"../family.toml\"\nname = \"b\"\n[target]\narch = \"arm\"\n",
        ),
    ]);
    let error = Board::read(dir.path().join("b/board.toml")).expect_err("an error");
    println!("{error}");
    assert_names(
        &error,
        &dir.path().join("family.toml"),
        "kernel.source",
        "no source named `nope`",
    );
}

#[test]
fn component_errors_name_file_and_key() {
    let dir = files(&[
        (
            "c.toml",
            "version = 1\nname = \"c\"\n[[install]]\nfrom = \"bin/x\"\nto = \"/usr/bin/x\"\n",
        ),
        (
            "d.toml",
            "version = 1\nname = \"d\"\n[[install]]\nfrom = { file = \"x\" }\nto = \"usr/bin/x\"\n",
        ),
        ("x", ""),
        (
            "e.toml",
            "version = 1\nname = \"e\"\n[[install]]\nfrom = { file = \"x\" }\nto = \"/x\"\nmode = \"9\"\n",
        ),
        (
            "f.toml",
            "version = 1\nname = \"f\"\n[[services]]\nunit = \"f.service\"\nenabled = true\n",
        ),
    ]);
    let error = Component::read(dir.path().join("c.toml")).expect_err("c");
    println!("{error}");
    assert_names(
        &error,
        &dir.path().join("c.toml"),
        "install[0].from",
        "no [build]",
    );
    let error = Component::read(dir.path().join("d.toml")).expect_err("d");
    println!("{error}");
    assert_names(
        &error,
        &dir.path().join("d.toml"),
        "install[0].to",
        "absolute",
    );
    let error = Component::read(dir.path().join("e.toml")).expect_err("e");
    println!("{error}");
    assert_names(
        &error,
        &dir.path().join("e.toml"),
        "install[0].mode",
        "octal",
    );
    let error = Component::read(dir.path().join("f.toml")).expect_err("f");
    println!("{error}");
    assert_names(
        &error,
        &dir.path().join("f.toml"),
        "services[0].enabled",
        "did you mean `services[0].enable`?",
    );
}

#[test]
fn product_errors_name_file_and_key() {
    let dir = files(&[
        (
            "p.toml",
            "version = 1\nname = \"p\"\n[components]\nnet = \"c.toml\"\n[[images]]\nname = \"i\"\nboard = \"b.toml\"\ncomponents = [\"nte\"]\nrecipe = \"r\"\n",
        ),
        (
            "q.toml",
            "version = 1\nname = \"q\"\n[[images]]\nname = \"i\"\nboard = \"b.toml\"\nrecipe = \"r\"\n[[images]]\nname = \"i\"\nboard = \"b.toml\"\nrecipe = \"r\"\n",
        ),
        (
            "r.toml",
            "version = 1\nname = \"r\"\n[[images]]\nname = \"i\"\nboard = \"gone.toml\"\nrecipe = \"r\"\n",
        ),
    ]);
    let error = Product::read(dir.path().join("p.toml")).expect_err("p");
    println!("{error}");
    assert_names(
        &error,
        &dir.path().join("p.toml"),
        "images[0].components[0]",
        "did you mean `net`?",
    );
    let error = Product::read(dir.path().join("q.toml")).expect_err("q");
    println!("{error}");
    assert_names(
        &error,
        &dir.path().join("q.toml"),
        "images[1].name",
        "another image",
    );
    // A board file that is not there: the error names that file.
    let error = Product::load(dir.path().join("r.toml")).expect_err("r");
    println!("{error}");
    assert_eq!(error.file, dir.path().join("gone.toml"));
    assert!(error.message.contains("cannot read"), "{error}");
}

#[test]
fn a_package_without_a_description_names_package_description() {
    let dir = files(&[(
        "c/component.toml",
        "version = 1\nname = \"c\"\n[package]\nversion = \"1\"\narchitecture = \"all\"\nmaintainer = \"m\"\n",
    )]);
    let path = dir.path().join("c/component.toml");
    let error = Component::read(&path).expect_err("an error");
    println!("{error}");
    assert_names(&error, &path, "package.description", "needs a description");
}
