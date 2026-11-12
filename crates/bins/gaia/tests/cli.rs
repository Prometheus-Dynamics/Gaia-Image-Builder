//! The `gaia` binary on a product it only plans: nothing is fetched or
//! built.

use std::path::Path;
use std::process::{Command, Output};

const URL: &str = "https://github.com/vasilakisfil/hello.service";
const COMMIT: &str = "3b9ee52ca0dbddcb64eb835e847f2059a8008ac5";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

fn product(dir: &Path) {
    write(
        &dir.join("boards/pc/board.toml"),
        "version = 1\nname = \"pc\"\n[target]\narch = \"x86_64\"\nos = \"linux\"\n",
    );
    write(
        &dir.join("components/hello/component.toml"),
        &format!(
            "version = 1\nname = \"hello\"\n[sources.src]\ngit = \"{URL}\"\nref = \"master\"\n[sources.vendor]\npath = \"vendor\"\n[sources.root]\nimage = \"docker.io/library/debian:trixie-slim\"\n[build]\nbuilder = \"rust\"\nsource = \"src\"\n[build.settings]\npackage = \"hello-world\"\nfeatures = \"\"\ntriple = \"x86_64-unknown-linux-musl\"\ncpu = \"\"\n"
        ),
    );
    std::fs::create_dir_all(dir.join("components/hello/vendor")).expect("mkdir");
    write(
        &dir.join("recipes/tarball.toml"),
        "version = 1\nname = \"tarball\"\n[stages]\ntar = \"gaia.compress.tar\"\n[inputs]\nbuild = [\"tar.tree\"]\n[outputs]\ntar = \"tar.archive\"\n",
    );
    write(
        &dir.join("product.toml"),
        "version = 1\nname = \"hello\"\n[components]\nhello = \"components/hello/component.toml\"\n[[images]]\nname = \"a\"\nboard = \"boards/pc/board.toml\"\ncomponents = [\"hello\"]\nrecipe = \"tarball\"\n",
    );
    write(
        &dir.join("gaia.lock"),
        &format!(
            "version = 1\n\n[source.\"hello.src\"]\nkind = \"git\"\nurl = \"{URL}\"\nref = \"master\"\ncommit = \"{COMMIT}\"\n[source.\"hello.root\"]\nkind = \"image\"\nreference = \"docker.io/library/debian:trixie-slim\"\ndigest = \"sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2\"\n"
        ),
    );
}

fn gaia(dir: &Path, args: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_gaia"))
        .args(args)
        .current_dir(dir)
        .env("GAIA_STORE", dir.join("store"))
        .env_remove("XDG_CACHE_HOME")
        .output()
        .expect("runs");
    println!("$ gaia {}", args.join(" "));
    print!("{}", String::from_utf8_lossy(&out.stdout));
    eprint!("{}", String::from_utf8_lossy(&out.stderr));
    out
}

#[test]
fn plan_prints_the_graph_and_runs_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    product(dir.path());
    let out = gaia(
        dir.path(),
        &["plan", "product.toml", "--tool", "toolchain=/opt/rust"],
    );
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).expect("utf-8");
    for want in [
        "tool    toolchain = /opt/rust",
        "graph: 5 nodes, 5 edges",
        "hello.src.tree -> hello.build.source",
        "hello.root.rootfs -> hello.build.root",
        "a::tar.archive -> host.a.tar",
        "plan: 5 steps",
        "a.tar  gaia:artifact",
    ] {
        assert!(text.contains(want), "no `{want}` in:\n{text}");
    }
    assert!(!dir.path().join("store").exists(), "plan touched the store");

    let out = gaia(dir.path(), &["plan", "product.toml", "--json"]);
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(json["graph"]["format"], "daedalus.graph");
    assert_eq!(json["plan"]["units"].as_array().map(Vec::len), Some(5));
}

#[test]
fn wrong_input_exits_2_naming_the_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    product(dir.path());
    std::fs::remove_file(dir.path().join("gaia.lock")).expect("rm");
    let out = gaia(dir.path(), &["build", "product.toml"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("gaia.lock"));

    let out = gaia(dir.path(), &["plan", "product.toml", "--frob"]);
    assert_eq!(out.status.code(), Some(2));

    let out = gaia(dir.path(), &["show", "a.tar"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no build recorded"));

    let out = gaia(dir.path(), &["clean", "--dry-run"]);
    assert!(out.status.success());
}

#[test]
fn lock_check_names_what_the_lock_lacks() {
    let dir = tempfile::tempdir().expect("tempdir");
    product(dir.path());
    // The fixture's lock pins the git source only; the vendor directory is
    // a source too.
    let out = gaia(dir.path(), &["lock", "product.toml", "--check"]);
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("not in the lock: hello.vendor"), "{text}");
    let out = gaia(dir.path(), &["lock", "product.toml", "--update", "nope"]);
    assert_eq!(out.status.code(), Some(2));
}

/// Copies `from` into `to`, keeping modes.
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

/// The `key` line `gaia show` prints for an output.
fn shown_key(dir: &Path, output: &str) -> String {
    let out = gaia(dir, &["show", output]);
    assert!(out.status.success());
    String::from_utf8(out.stdout)
        .expect("utf-8")
        .lines()
        .find_map(|line| line.split_once(", key ").map(|(_, key)| key.to_string()))
        .expect("a step key")
}

#[test]
fn a_package_is_built_from_component_files_with_its_control_fed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../examples/packages");
    copy_dir(&examples, dir.path());
    let _ = std::fs::remove_file(dir.path().join("gaia.lock"));
    assert!(gaia(dir.path(), &["lock", "product.toml"]).status.success());

    // The control data is a host input, and the plan shows it.
    let out = gaia(dir.path(), &["plan", "product.toml"]);
    let text = String::from_utf8(out.stdout).expect("utf-8");
    for want in [
        "host.robot-status.control -> robot-status::pkg.control",
        "robot-status.control  gaia:package-control  robot-status 1.2.0 aarch64, depends busybox",
        "robot-status.ipk  gaia:artifact",
    ] {
        assert!(text.contains(want), "no `{want}` in:\n{text}");
    }
    let out = gaia(dir.path(), &["plan", "product.toml", "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(json["host_inputs"][0]["port"], "robot-status.control");
    assert_eq!(json["host_inputs"][0]["value"]["version"], "1.2.0");

    assert!(
        gaia(dir.path(), &["build", "product.toml"])
            .status
            .success()
    );
    let first = shown_key(dir.path(), "robot-status.ipk");

    // Another version is another step: the fed value is in the key.
    let component = dir.path().join("components/robot-status/component.toml");
    let text = std::fs::read_to_string(&component).expect("read");
    std::fs::write(&component, text.replace("1.2.0", "1.2.1")).expect("write");
    let out = gaia(dir.path(), &["build", "product.toml"]);
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("made")
            || String::from_utf8_lossy(&out.stdout).contains("made"),
        "the package was made again"
    );
    let second = shown_key(dir.path(), "robot-status.ipk");
    assert_ne!(first, second, "a changed [package] kept the key");
}
