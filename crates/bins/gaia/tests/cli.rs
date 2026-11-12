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
