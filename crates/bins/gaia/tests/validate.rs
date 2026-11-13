//! `gaia validate`: description files checked on their own, with no lock,
//! no store and no tools. The examples pass; a bad file fails naming its key.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("read_dir") {
        let entry = entry.expect("entry");
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type().expect("type").is_dir() {
            copy_dir(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).expect("copy");
        }
    }
}

/// The examples, copied so a test can change them.
fn examples() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    copy_dir(&repo().join("examples"), &dir.path().join("examples"));
    dir
}

/// Runs with an empty PATH and a store that does not exist: validate needs
/// neither.
fn validate(dir: &Path, args: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_gaia"))
        .arg("validate")
        .args(args)
        .current_dir(dir)
        .env("PATH", "")
        .env("GAIA_STORE", dir.join("no-store"))
        .output()
        .expect("runs");
    println!("$ gaia validate {}", args.join(" "));
    print!("{}", String::from_utf8_lossy(&out.stdout));
    eprint!("{}", String::from_utf8_lossy(&out.stderr));
    out
}

fn edit(path: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(path).expect("read");
    assert!(text.contains(from), "{} has no {from:?}", path.display());
    std::fs::write(path, text.replace(from, to)).expect("write");
}

const BOARD: &str = "examples/boards/zero-3e/board.toml";
const FAMILY: &str = "examples/boards/rk35xx/family.toml";
const COMPONENT: &str = "examples/components/usb-gadget-network/component.toml";
const PRODUCT: &str = "examples/products/zero-3e-demo/product.toml";

#[test]
fn each_kind_of_example_is_ok_and_named() {
    let dir = examples();
    let out = validate(dir.path(), &[BOARD, FAMILY, COMPONENT, PRODUCT]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        [
            format!("ok  board  {BOARD}"),
            format!("ok  family  {FAMILY}"),
            format!("ok  component  {COMPONENT}"),
            format!("ok  product  {PRODUCT}"),
        ]
    );
    assert!(!dir.path().join("no-store").exists(), "no store is made");
}

#[test]
fn an_unknown_key_fails_with_the_file_the_key_and_the_one_meant() {
    let dir = examples();
    edit(&dir.path().join(BOARD), "device_tree =", "device_tre =");
    let out = validate(dir.path(), &[BOARD]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(BOARD), "{err}");
    assert!(err.contains("kernel.device_tre"), "{err}");
    assert!(err.contains("did you mean `kernel.device_tree`?"), "{err}");
}

#[test]
fn a_source_that_is_not_named_fails_at_its_key() {
    let dir = examples();
    edit(
        &dir.path().join(BOARD),
        "source = \"rkbin\"",
        "source = \"rkbim\"",
    );
    let out = validate(dir.path(), &[BOARD]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no source named `rkbim`"), "{err}");
    assert!(err.contains("did you mean `rkbin`?"), "{err}");
}

#[test]
fn a_product_is_checked_with_the_files_it_names() {
    let dir = examples();
    // The product's board is bad: the product fails, naming the board file.
    edit(&dir.path().join(BOARD), "cpu = ", "cpus = ");
    let out = validate(dir.path(), &[PRODUCT]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(BOARD) && err.contains("target.cpus"), "{err}");
    // And a component the product does not have.
    let dir = examples();
    edit(
        &dir.path().join(PRODUCT),
        "components = [\"usb-gadget-network\", \"extlinux\"]",
        "components = [\"usb-gadget\", \"extlinux\"]",
    );
    let out = validate(dir.path(), &[PRODUCT]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("images[0].components[0]"), "{err}");
}

#[test]
fn it_stops_at_the_first_bad_file() {
    let dir = examples();
    edit(&dir.path().join(FAMILY), "[kernel]", "[kernal]");
    let out = validate(dir.path(), &[COMPONENT, FAMILY, PRODUCT]);
    assert_eq!(out.status.code(), Some(2));
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(text.lines().count(), 1, "one ok line, then the stop");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(FAMILY) && err.contains("kernal"), "{err}");
}

#[test]
fn a_family_that_extends_is_refused_and_kind_overrides_the_guess() {
    let dir = examples();
    // As a family, the board file is refused: it extends.
    let out = validate(dir.path(), &["--kind", "family", BOARD]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("a family file extends nothing"), "{err}");
    // As a board, the family has no name.
    let out = validate(dir.path(), &["--kind", "board", FAMILY]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("name") && err.contains("missing"), "{err}");
}

#[test]
fn no_file_a_missing_file_and_a_bad_kind_are_errors() {
    let dir = examples();
    assert_eq!(validate(dir.path(), &[]).status.code(), Some(2));
    let out = validate(dir.path(), &["examples/nothing.toml"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot read"));
    let out = validate(dir.path(), &["--kind", "recipe", BOARD]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--kind takes"));
}
