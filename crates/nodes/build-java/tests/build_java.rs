//! A library jar and an app using it, each built twice in two fresh
//! stores: the same bytes, and the app runs from the output tree.
//!
//! The root is `debian:trixie-slim` and the JDK is the one in
//! `eclipse-temurin:25-jdk` (`/opt/java/openjdk`), both pinned by their
//! index digests and pulled by the OCI source node (this needs the
//! network). Where bubblewrap or user namespaces are missing the tests say
//! SKIPPED, unless `GAIA_REQUIRE_SANDBOX=1`, when they fail. Where a pull
//! is missing they say SKIPPED, unless `GAIA_REQUIRE_TOOLS=1`, when they
//! fail.

use gaia_build_java::build_java;
use gaia_step::{Ctx, logs_dir};
use gaia_store::{Store, file_digest};
use gaia_types::Tree;
use std::path::{Path, PathBuf};

const JDK_IMAGE: &str = "docker.io/library/eclipse-temurin:25-jdk";
const JDK_DIGEST: &str = "sha256:1a3ebcfa0e69c6f33e2e8a560dd875b2278bac11b36eab21a252580fc8a325b0";

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

/// The root image and the toolchain image, pulled once into a store of
/// their own; each build copies the root into its fresh store.
struct Images {
    _cache: tempfile::TempDir,
    root: PathBuf,
    toolchain: PathBuf,
}

/// The images, or `None` (a skip) where the sandbox does not run or they
/// do not pull. The toolchain is the directory `within` the toolchain
/// image.
fn images(toolchain_image: &str, toolchain_digest: &str, within: &str) -> Option<Images> {
    if !sandbox_here() {
        return None;
    }
    let cache = tempfile::tempdir().expect("tempdir");
    let store = cache.path().join("store");
    let root = pull(&store, IMAGE, IMAGE_DIGEST)?;
    let toolchain = pull(&store, toolchain_image, toolchain_digest)?.join(within);
    assert!(toolchain.is_dir(), "{toolchain_image} has no {within}");
    Some(Images {
        _cache: cache,
        root,
        toolchain,
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

fn stored(store: &Path, dir: &Path) -> Tree {
    std::fs::create_dir_all(dir).expect("mkdir");
    let store = Store::open(store).expect("store");
    Tree {
        object: store.put_tree(dir).expect("put").into(),
        entries: Vec::new(),
    }
}

/// Builds the library, then the app against it, in a fresh store under
/// `root`. Gives each built jar's digest and size, and the app's tree.
fn build(images: &Images, root: &Path) -> (Vec<(String, u64, String)>, PathBuf) {
    let store = root.join("store");
    let s = store.display().to_string();
    let lib_src = root.join("greet");
    write(
        &lib_src.join("src/main/java/dev/gaia/greet/Greet.java"),
        "package dev.gaia.greet;\n\npublic final class Greet {\n    public static String hello(String who) { return \"hello, \" + who; }\n}\n",
    );
    write(
        &lib_src.join("src/main/resources/greet.properties"),
        "greeting=hello\n",
    );
    let app_src = root.join("app");
    write(
        &app_src.join("src/main/java/dev/gaia/app/Main.java"),
        "package dev.gaia.app;\n\nimport dev.gaia.greet.Greet;\n\npublic final class Main {\n    public static void main(String[] args) { System.out.println(Greet.hello(\"image\")); }\n}\n",
    );
    let ctx = Ctx::new();
    let j = images.toolchain.display().to_string();
    let (_, greet) = build_java(
        &s,
        BWRAP,
        stored(&store, &images.root),
        &j,
        stored(&store, &lib_src),
        stored(&store, &root.join("none")),
        "greet",
        17,
        "",
        &ctx,
    )
    .expect("library");
    // The app's dependency jars: what the lock would pin, here the library.
    let libs = root.join("libs");
    std::fs::create_dir_all(&libs).expect("mkdir");
    let handle = Store::open(&store).expect("store");
    std::fs::copy(
        greet.object.path_in(&handle).expect("in"),
        libs.join("greet.jar"),
    )
    .expect("copy");
    let (tree, app) = build_java(
        &s,
        BWRAP,
        stored(&store, &images.root),
        &j,
        stored(&store, &app_src),
        stored(&store, &libs),
        "app",
        17,
        "dev.gaia.app.Main",
        &ctx,
    )
    .expect("app");
    let list = [("greet.jar", &greet), ("app.jar", &app)]
        .map(|(n, a)| {
            let p = a.object.path_in(&handle).expect("in");
            (
                n.to_string(),
                std::fs::metadata(&p).expect("size").len(),
                file_digest(&p).expect("digest").to_string(),
            )
        })
        .to_vec();
    (list, tree.object.path_in(&handle).expect("in"))
}

#[test]
fn a_small_project_builds_offline_twice_to_the_same_digest() {
    let Some(images) = images(JDK_IMAGE, JDK_DIGEST, "opt/java/openjdk") else {
        return;
    };
    let jdk = &images.toolchain;
    let (a, b) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let (first, tree) = build(&images, a.path());
    let (second, _) = build(&images, b.path());
    println!("JDK {JDK_IMAGE} (sandboxed, root {IMAGE_DIGEST})");
    for (name, size, digest) in &first {
        println!("  {name:<10} {size:>6} bytes  {digest}");
    }
    assert_eq!(first, second, "two stores, the same bytes");
    let store = Store::open(a.path().join("store")).expect("store");
    all_sandboxed(&logs_dir(&store), "/toolchain/bin/javac");

    // The output tree runs: the app jar and its dependency side by side.
    let mut listed: Vec<String> = std::fs::read_dir(tree.join("lib"))
        .expect("lib")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    listed.sort();
    assert_eq!(listed, ["app.jar", "greet.jar"]);
    let out = std::process::Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(format!("{}/lib/*", tree.display()))
        .arg("dev.gaia.app.Main")
        .output()
        .expect("java");
    let text = String::from_utf8_lossy(&out.stdout);
    println!("runs: {}", text.trim());
    assert_eq!(text.trim(), "hello, image");
    // The manifest names the entry point.
    let manifest = std::process::Command::new(jdk.join("bin/jar"))
        .arg("--extract")
        .arg("--file")
        .arg(tree.join("lib/app.jar"))
        .arg("META-INF/MANIFEST.MF")
        .current_dir(a.path())
        .status()
        .expect("jar");
    assert!(manifest.success());
    let text = std::fs::read_to_string(a.path().join("META-INF/MANIFEST.MF")).expect("manifest");
    assert!(text.contains("Main-Class: dev.gaia.app.Main"), "{text}");
}

#[test]
fn a_missing_class_is_a_compile_error_with_its_output() {
    let Some(images) = images(JDK_IMAGE, JDK_DIGEST, "opt/java/openjdk") else {
        return;
    };
    let root = tempfile::tempdir().expect("tempdir");
    let store = root.path().join("store");
    let src = root.path().join("src");
    write(
        &src.join("Main.java"),
        "public final class Main { Missing m; }\n",
    );
    let error = build_java(
        &store.display().to_string(),
        BWRAP,
        stored(&store, &images.root),
        &images.toolchain.display().to_string(),
        stored(&store, &src),
        stored(&store, &root.path().join("none")),
        "main",
        0,
        "",
        &Ctx::new(),
    )
    .expect_err("fails");
    let text = format!("{error} {}", error.stderr_tail().join("\n"));
    println!("{text}");
    assert!(text.contains("Missing"), "{text}");
}
