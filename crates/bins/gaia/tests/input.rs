//! A named input: `gaia plan --input NAME=PATH` gives a component's
//! `{ input = NAME }` install entry its file. The temp copy of the example
//! product gets a component that takes one; nothing real runs (tools are
//! named, not run) and no key is committed.

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

fn edit(path: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(path).expect("read");
    assert!(text.contains(from), "{} has no {from:?}", path.display());
    std::fs::write(path, text.replacen(from, to, 1)).expect("write");
}

/// The examples and recipes in a temp dir, plus a component `bench-key`
/// that installs the named input `bench-key` as root's authorized keys, and
/// a key file in `keys/`. The product takes the component.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    copy_dir(&repo().join("examples"), &dir.path().join("examples"));
    copy_dir(&repo().join("recipes"), &dir.path().join("recipes"));
    let component = dir.path().join("examples/components/bench-key");
    std::fs::create_dir_all(&component).expect("mkdir");
    std::fs::write(
        component.join("component.toml"),
        "version = 1\nname = \"bench-key\"\n\n[[install]]\nfrom = { input = \"bench-key\" }\nto = \"/root/.ssh/authorized_keys\"\nmode = \"0600\"\n",
    )
    .expect("write");
    let product = dir
        .path()
        .join("examples/products/zero-3e-demo/product.toml");
    edit(
        &product,
        "usb-gadget-network = \"../../components/usb-gadget-network/component.toml\"",
        "bench-key = \"../../components/bench-key/component.toml\"\nusb-gadget-network = \"../../components/usb-gadget-network/component.toml\"",
    );
    edit(
        &product,
        "components = [\"usb-gadget-network\"",
        "components = [\"bench-key\", \"usb-gadget-network\"",
    );
    let keys = dir.path().join("keys");
    std::fs::create_dir_all(&keys).expect("mkdir");
    std::fs::write(keys.join("bench.pub"), "ssh-ed25519 AAAA bench\n").expect("write");
    dir
}

/// `gaia <plan|build> <product> ... extra`, with the tools named as the OS
/// test does.
fn gaia(dir: &Path, command: &str, extra: &[&str]) -> Output {
    let mut args = vec![command, "examples/products/zero-3e-demo/product.toml"];
    args.extend([
        "--recipes",
        "recipes",
        "--tool",
        "make=/usr/bin/make",
        "--tool",
        "mkfs_erofs=/usr/bin/mkfs.erofs",
        "--tool",
        "veritysetup=/usr/sbin/veritysetup",
        "--tool",
        "bwrap=/usr/bin/bwrap",
        "--tool",
        "toolchain=/opt/rust",
        "--tool",
        "mke2fs=/usr/sbin/mke2fs",
        "--tool",
        "debugfs=/usr/sbin/debugfs",
    ]);
    args.extend(extra);
    let out = Command::new(env!("CARGO_BIN_EXE_gaia"))
        .args(&args)
        .current_dir(dir)
        .env("GAIA_STORE", dir.join("store"))
        .env("GAIA_JOBS", "4")
        .env_remove("GAIA_CPUS")
        .env_remove("GAIA_BUILDROOT_DL")
        .env_remove("XDG_CACHE_HOME")
        .output()
        .expect("runs");
    println!("$ gaia {}", args.join(" "));
    print!("{}", String::from_utf8_lossy(&out.stdout));
    eprint!("{}", String::from_utf8_lossy(&out.stderr));
    out
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

const INPUT_LABEL: &str = "zero-3e-demo.bench-key.input.bench-key";

#[test]
fn the_plan_has_the_input_node_and_its_placement() {
    let dir = project();
    let out = gaia(dir.path(), "plan", &["--input", "bench-key=keys/bench.pub"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    let node = text
        .lines()
        .find(|l| l.contains(INPUT_LABEL))
        .expect("the node line");
    assert!(node.contains("gaia.source.input"), "{node}");
    let edge = format!("{INPUT_LABEL}.tree -> zero-3e-demo.bench-key.place0.from");
    assert!(text.contains(&edge), "the plan has no edge {edge:?}");
}

#[test]
fn the_json_plan_has_the_path_and_name_and_the_destination() {
    let dir = project();
    let out = gaia(
        dir.path(),
        "plan",
        &["--input", "bench-key=keys/bench.pub", "--json"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let both: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    let nodes = both["graph"]["graph"]["nodes"].as_array().expect("nodes");
    let node = nodes
        .iter()
        .find(|n| n["label"] == INPUT_LABEL)
        .expect("the input node");
    assert_eq!(node["id"], "gaia.source.input");
    assert_eq!(node["consts"]["name"]["value"], "bench-key");
    let path = node["consts"]["path"]["value"].as_str().expect("a path");
    assert!(path.ends_with("keys/bench.pub"), "{path}");
    let placements = both["host_inputs"]
        .as_array()
        .expect("host inputs")
        .iter()
        .find(|h| h["port"] == "zero-3e-demo.bench-key.placements0")
        .expect("the placements");
    assert_eq!(placements["value"]["entries"][0]["from"], "bench-key");
    assert_eq!(
        placements["value"]["entries"][0]["to"],
        "/root/.ssh/authorized_keys"
    );
    assert_eq!(placements["value"]["entries"][0]["mode"], 0o600);
}

#[test]
fn a_plan_without_the_input_fails_naming_it() {
    let dir = project();
    let out = gaia(dir.path(), "plan", &[]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("bench-key/component.toml"), "{err}");
    assert!(err.contains("install[0].from.input"), "{err}");
    assert!(
        err.contains(
            "the named input `bench-key` is not given; give it with --input bench-key=<path>"
        ),
        "{err}"
    );
}

#[test]
fn a_build_without_the_input_fails_the_same_way() {
    let dir = project();
    let out = gaia(dir.path(), "build", &[]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("give it with --input bench-key=<path>"));
}

#[test]
fn an_input_that_is_not_a_file_fails_naming_it_and_the_path() {
    let dir = project();
    for path in ["keys", "keys/none.pub"] {
        let out = gaia(
            dir.path(),
            "plan",
            &["--input", &format!("bench-key={path}")],
        );
        assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
        let err = stderr(&out);
        assert!(
            err.contains(&format!("--input bench-key: {path} is not a regular file")),
            "{err}"
        );
    }
}

#[test]
fn an_unused_input_fails_and_lists_the_real_ones() {
    let dir = project();
    let out = gaia(
        dir.path(),
        "plan",
        &[
            "--input",
            "bench-key=keys/bench.pub",
            "--input",
            "bench-kye=keys/bench.pub",
        ],
    );
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("--input bench-kye="), "{err}");
    assert!(
        err.contains(
            "names no input the product's components take; the inputs it takes are bench-key"
        ),
        "{err}"
    );
}

#[test]
fn a_malformed_input_flag_is_a_usage_error() {
    let dir = project();
    let out = gaia(dir.path(), "plan", &["--input", "bench-key"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("--input takes NAME=PATH, not `bench-key`"));
}

#[test]
fn validate_needs_no_input() {
    let dir = project();
    let product = Command::new(env!("CARGO_BIN_EXE_gaia"))
        .args([
            "validate",
            "examples/products/zero-3e-demo/product.toml",
            "--recipes",
            "recipes",
        ])
        .current_dir(dir.path())
        .output()
        .expect("runs");
    assert!(product.status.success(), "{}", stderr(&product));
    let component = Command::new(env!("CARGO_BIN_EXE_gaia"))
        .args(["validate", "examples/components/bench-key/component.toml"])
        .current_dir(dir.path())
        .output()
        .expect("runs");
    assert!(component.status.success(), "{}", stderr(&component));
    assert!(
        stdout(&component).contains("ok  component"),
        "{}",
        stdout(&component)
    );
}
