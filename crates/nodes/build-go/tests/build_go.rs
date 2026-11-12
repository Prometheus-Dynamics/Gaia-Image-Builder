//! A small module with a vendored dependency, built for aarch64 twice in
//! two fresh stores: the same digests.
//!
//! The root is `debian:trixie-slim`, pinned by its index digest and pulled
//! by the OCI source node (this needs the network). The Go root is this
//! host's (`go env GOROOT`; Go's tools are static), bound read-only. Where
//! bubblewrap or user namespaces are missing the tests say SKIPPED, unless
//! `GAIA_REQUIRE_SANDBOX=1`, when they fail. Where the pull or Go is
//! missing they say SKIPPED, unless `GAIA_REQUIRE_TOOLS=1`, when they fail.

use gaia_build_go::{build_go, go_target};
use gaia_step::{Ctx, logs_dir};
use gaia_store::{Store, file_digest};
use gaia_types::Tree;
use std::path::{Path, PathBuf};
use std::process::Command;

const BWRAP: &str = "/usr/bin/bwrap";
const CURL: &str = "/usr/bin/curl";
/// The root: the index of `debian:trixie-slim`, pinned, for linux/amd64.
const IMAGE: &str = "docker.io/library/debian:trixie-slim";
const IMAGE_DIGEST: &str =
    "sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2";

fn required(var: &str) -> bool {
    std::env::var_os(var).is_some()
}

/// Whether bubblewrap runs here. Missing, it is a skip unless required.
fn sandbox_here() -> bool {
    let ok = Path::new(BWRAP).exists()
        && std::process::Command::new(BWRAP)
            .args(["--unshare-all", "--ro-bind", "/", "/", "true"])
            .status()
            .is_ok_and(|s| s.success());
    if !ok {
        assert!(
            !required("GAIA_REQUIRE_SANDBOX"),
            "GAIA_REQUIRE_SANDBOX is set, and no sandbox runs here"
        );
        println!("SKIPPED: no bubblewrap with user namespaces here");
    }
    ok
}

/// `image`, pulled into `store` by digest: its root directory. Missing the
/// network, a skip unless required.
fn pull(store: &Path, image: &str, digest: &str) -> Option<PathBuf> {
    let store_text = store.display().to_string();
    match gaia_source_oci::source(
        &store_text,
        CURL,
        image,
        digest,
        "linux/amd64",
        "",
        &Ctx::new(),
    ) {
        Ok((tree, _)) => {
            let handle = Store::open(store).expect("store");
            Some(tree.object.path_in(&handle).expect("in the store"))
        }
        Err(error) => {
            assert!(
                !required("GAIA_REQUIRE_TOOLS"),
                "GAIA_REQUIRE_TOOLS is set, and {image} did not pull: {error}"
            );
            println!("SKIPPED: {image} did not pull: {error}");
            None
        }
    }
}

/// The root image, pulled once into a store of its own (each build copies
/// it into its fresh store), and this host's Go root.
struct Images {
    _cache: tempfile::TempDir,
    root: PathBuf,
    goroot: PathBuf,
}

/// The images, or `None` (a skip) where the sandbox does not run, the root
/// does not pull or there is no Go here.
fn images() -> Option<Images> {
    if !sandbox_here() {
        return None;
    }
    let goroot = Command::new("go")
        .args(["env", "GOROOT"])
        .env("GOTOOLCHAIN", "local")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| PathBuf::from(s.trim()))
        .filter(|root| root.join("bin/go").is_file());
    let Some(goroot) = goroot else {
        assert!(
            !required("GAIA_REQUIRE_TOOLS"),
            "GAIA_REQUIRE_TOOLS is set, and there is no Go toolchain here"
        );
        println!("SKIPPED: no Go toolchain here");
        return None;
    };
    let cache = tempfile::tempdir().expect("tempdir");
    let root = pull(&cache.path().join("store"), IMAGE, IMAGE_DIGEST)?;
    Some(Images {
        _cache: cache,
        root,
        goroot,
    })
}

/// The command lines of every tool run whose logs are under `logs`.
fn tools_run(logs: &Path) -> Vec<String> {
    let mut tools = Vec::new();
    for dir in std::fs::read_dir(logs).expect("logs") {
        let dir = dir.expect("entry").path();
        if !dir.is_dir() {
            continue;
        }
        for log in std::fs::read_dir(dir).expect("step logs") {
            let text = std::fs::read_to_string(log.expect("log").path()).expect("read");
            tools.extend(
                text.lines()
                    .filter_map(|l| l.strip_prefix("# tool ").map(String::from)),
            );
        }
    }
    tools
}

/// Every tool ran under bubblewrap, and one of them ran `wanted`.
fn all_sandboxed(logs: &Path, wanted: &str) {
    let tools = tools_run(logs);
    println!("run log tools:");
    for tool in &tools {
        println!("  {}", tool.chars().take(200).collect::<String>());
    }
    assert!(
        tools.iter().all(|t| t.starts_with(BWRAP)),
        "a tool ran outside the sandbox: {tools:?}"
    );
    assert!(
        tools.iter().any(|t| t.contains(wanted)),
        "no {wanted} in the run logs: {tools:?}"
    );
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

/// A module with two commands and a vendored dependency.
fn module(dir: &Path) {
    write(
        &dir.join("go.mod"),
        "module example.com/hello\n\ngo 1.22\n\nrequire example.org/tiny v1.0.0\n",
    );
    write(
        &dir.join("cmd/hello/main.go"),
        "package main\n\nimport (\n\t\"fmt\"\n\t\"example.org/tiny\"\n)\n\nfunc main() { fmt.Println(\"hello\", tiny.Answer) }\n",
    );
    write(
        &dir.join("cmd/bye/main.go"),
        "package main\n\nimport \"fmt\"\n\nfunc main() { fmt.Println(\"bye\") }\n",
    );
    write(
        &dir.join("vendor/modules.txt"),
        "# example.org/tiny v1.0.0\n## explicit\nexample.org/tiny\n",
    );
    write(
        &dir.join("vendor/example.org/tiny/tiny.go"),
        "package tiny\n\nconst Answer = 42\n",
    );
}

fn stored(store: &Path, dir: &Path) -> Tree {
    let store = Store::open(store).expect("store");
    Tree {
        object: store.put_tree(dir).expect("put").into(),
        entries: Vec::new(),
    }
}

/// Builds in a fresh store under `root`: each built file's path, size and
/// digest, and the output's `bin` directory.
fn build(root: &Path, images: &Images, triple: &str) -> (Vec<(String, u64, String)>, PathBuf) {
    let (src, modules, store) = (root.join("src"), root.join("modules"), root.join("store"));
    module(&src);
    std::fs::create_dir_all(&modules).expect("mkdir");
    let (tree, artifacts) = build_go(
        &store.display().to_string(),
        BWRAP,
        stored(&store, &images.root),
        &images.goroot.display().to_string(),
        stored(&store, &src),
        stored(&store, &modules),
        "./cmd/...",
        triple,
        &Ctx::new(),
    )
    .expect("builds");
    assert_eq!(artifacts.len(), 2);
    let s = Store::open(&store).expect("store");
    let bin = tree.object.path_in(&s).expect("in").join("bin");
    let mut files: Vec<_> = std::fs::read_dir(&bin)
        .expect("read")
        .flatten()
        .map(|e| {
            let p = e.path();
            (
                format!("bin/{}", e.file_name().to_string_lossy()),
                std::fs::metadata(&p).expect("size").len(),
                file_digest(&p).expect("digest").to_string(),
            )
        })
        .collect();
    files.sort();
    (files, bin)
}

#[test]
fn a_small_module_builds_for_aarch64_twice_to_the_same_digest() {
    let Some(images) = images() else { return };
    let (a, b) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let triple = "aarch64-unknown-linux-musl";
    let (first, bin) = build(a.path(), &images, triple);
    let (second, _) = build(b.path(), &images, triple);
    println!("{triple} (linux/arm64, sandboxed, root {IMAGE_DIGEST}):");
    for (path, size, digest) in &first {
        println!("  {path:<10} {size:>8} bytes  {digest}");
    }
    assert_eq!(first, second, "two stores, the same bytes");
    let names: Vec<&str> = first.iter().map(|f| f.0.as_str()).collect();
    assert_eq!(names, ["bin/bye", "bin/hello"]);
    let store = Store::open(a.path().join("store")).expect("store");
    all_sandboxed(&logs_dir(&store), "/toolchain/bin/go build");
    // Each is an aarch64 ELF (e_machine 183).
    let machines: Vec<u16> = std::fs::read_dir(&bin)
        .expect("bin")
        .map(|e| {
            let bytes = std::fs::read(e.expect("entry").path()).expect("read");
            u16::from_le_bytes([bytes[18], bytes[19]])
        })
        .collect();
    assert!(
        !machines.is_empty() && machines.iter().all(|m| *m == 183),
        "{machines:?}"
    );
}

#[test]
fn the_host_build_runs() {
    let Some(images) = images() else { return };
    let dir = tempfile::tempdir().expect("tempdir");
    let (_, bin) = build(dir.path(), &images, "x86_64-unknown-linux-gnu");
    let out = Command::new(bin.join("hello")).output().expect("runs");
    let text = String::from_utf8_lossy(&out.stdout);
    println!("runs: {}", text.trim());
    assert_eq!(text.trim(), "hello 42");
}

#[test]
fn triples_map_to_go_targets() {
    assert_eq!(
        go_target("aarch64-unknown-linux-musl").expect("ok"),
        ("linux".into(), "arm64".into(), None)
    );
    assert_eq!(
        go_target("armv7-unknown-linux-gnueabihf").expect("ok"),
        ("linux".into(), "arm".into(), Some("7".into()))
    );
    assert_eq!(
        go_target("riscv64gc-unknown-linux-gnu").expect("ok").1,
        "riscv64"
    );
    assert!(go_target("thumbv8m.main-none-eabihf").is_err());
}
