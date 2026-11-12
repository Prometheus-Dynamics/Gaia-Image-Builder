//! A small app with one locked dependency from an npm cache, built twice in
//! two fresh stores: the same bundle and package, and the bundle runs.
//!
//! The root is `debian:trixie-slim` and Node.js is the upstream release in
//! `node:22-trixie-slim` (`/usr/local`), both pinned by their index digests
//! and pulled by the OCI source node (this needs the network). The fixtures
//! (the dependency's tarball and the npm cache) are made with the same
//! Node.js, run here. Where bubblewrap or user namespaces are missing the
//! tests say SKIPPED, unless `GAIA_REQUIRE_SANDBOX=1`, when they fail.
//! Where a pull is missing they say SKIPPED, unless `GAIA_REQUIRE_TOOLS=1`,
//! when they fail.

use gaia_build_node::build_node;
use gaia_step::{Ctx, logs_dir};
use gaia_store::{Store, file_digest, tree_digest};
use gaia_types::Tree;
use std::path::{Path, PathBuf};
use std::process::Command;

const NODE_IMAGE: &str = "docker.io/library/node:22-trixie-slim";
const NODE_DIGEST: &str = "sha256:68c76cfad89921b927eae28ba21407475e29d0ce450afcdbe70c1532db042b64";

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

fn run(home: &Path, args: &[&str], cwd: &Path, scratch: &Path) -> String {
    let out = Command::new(home.join("bin").join(args[0]))
        .args(&args[1..])
        .current_dir(cwd)
        .env_clear()
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", home.join("bin").display()),
        )
        .env("HOME", scratch.join("home"))
        .output()
        .expect(args[0]);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf8")
}

/// The app's source and an npm cache holding its one dependency, as a
/// source node would give them from the lock.
fn inputs(home: &Path, dir: &Path) -> (PathBuf, PathBuf) {
    let tiny = dir.join("tiny");
    write(
        &tiny.join("package.json"),
        r#"{"name":"@gaia-test/tiny","version":"1.0.0","main":"index.js"}"#,
    );
    write(&tiny.join("index.js"), "module.exports = () => 42;\n");
    run(
        home,
        &[
            "npm",
            "pack",
            "--ignore-scripts",
            "--pack-destination",
            dir.to_str().expect("utf8"),
        ],
        &tiny,
        dir,
    );
    let tgz = dir.join("gaia-test-tiny-1.0.0.tgz");
    let integrity = run(
        home,
        &[
            "node",
            "-e",
            "const c=require('crypto'),f=require('fs');process.stdout.write('sha512-'+c.createHash('sha512').update(f.readFileSync(process.argv[1])).digest('base64'))",
            tgz.to_str().expect("utf8"),
        ],
        dir,
        dir,
    );
    let cache = dir.join("cache");
    run(
        home,
        &[
            "npm",
            "cache",
            "add",
            tgz.to_str().expect("utf8"),
            "--cache",
            cache.to_str().expect("utf8"),
        ],
        dir,
        dir,
    );
    let app = dir.join("app");
    write(
        &app.join("package.json"),
        r#"{"name":"app","version":"1.0.0","bin":{"app":"dist/app.js"},"files":["dist"],"dependencies":{"@gaia-test/tiny":"1.0.0"},"scripts":{"build":"node build.js"}}"#,
    );
    write(
        &app.join("build.js"),
        "const fs = require('fs');\nfs.mkdirSync('dist', { recursive: true });\nfs.writeFileSync('dist/app.js', \"#!/usr/bin/env node\\nconsole.log('answer ' + require('@gaia-test/tiny')());\\n\", { mode: 0o755 });\n",
    );
    write(
        &app.join("package-lock.json"),
        &format!(
            r#"{{"name":"app","version":"1.0.0","lockfileVersion":3,"requires":true,"packages":{{"":{{"name":"app","version":"1.0.0","dependencies":{{"@gaia-test/tiny":"1.0.0"}},"bin":{{"app":"dist/app.js"}}}},"node_modules/@gaia-test/tiny":{{"version":"1.0.0","resolved":"https://registry.npmjs.org/@gaia-test/tiny/-/tiny-1.0.0.tgz","integrity":"{integrity}"}}}}}}"#
        ),
    );
    (app, cache)
}

fn stored(store: &Path, dir: &Path) -> Tree {
    let store = Store::open(store).expect("store");
    Tree {
        object: store.put_tree(dir).expect("put").into(),
        entries: Vec::new(),
    }
}

#[test]
fn a_small_app_builds_offline_twice_to_the_same_digest() {
    let Some(images) = images(NODE_IMAGE, NODE_DIGEST, "usr/local") else {
        return;
    };
    let home = images.toolchain.clone();
    let shared = tempfile::tempdir().expect("tempdir");
    let (app, cache) = inputs(&home, shared.path());
    let mut results = Vec::new();
    for _ in 0..2 {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = dir.path().join("store");
        let (bundle, package) = build_node(
            &store.display().to_string(),
            BWRAP,
            stored(&store, &images.root),
            &home.display().to_string(),
            stored(&store, &app),
            stored(&store, &cache),
            "build",
            &Ctx::new(),
        )
        .expect("builds");
        let handle = Store::open(&store).expect("store");
        let bundle_dir = bundle.object.path_in(&handle).expect("in");
        let tgz = package.object.path_in(&handle).expect("in");
        let out = Command::new(home.join("bin/node"))
            .arg(bundle_dir.join("dist/app.js"))
            .output()
            .expect("node");
        let mut files = Vec::new();
        list(&bundle_dir, &bundle_dir, &mut files);
        results.push((
            tree_digest(&bundle_dir).expect("digest").to_string(),
            file_digest(&tgz).expect("digest").to_string(),
            std::fs::metadata(&tgz).expect("size").len(),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
            files,
        ));
        all_sandboxed(&logs_dir(&handle), "/toolchain/bin/npm ci");
        drop(dir);
    }
    let (bundle, package, size, ran, files) = &results[0];
    println!("node {NODE_IMAGE} (sandboxed, root {IMAGE_DIGEST})");
    println!("bundle   {bundle}");
    for f in files {
        println!("  {f}");
    }
    println!("package  {package} ({size} bytes)");
    println!("runs: {ran}");
    assert_eq!(
        results[0], results[1],
        "two stores, the same bundle and package"
    );
    assert_eq!(ran, "answer 42");
    // The built file and the locked dependency are in the bundle.
    assert!(files.iter().any(|f| f == "dist/app.js"));
    assert!(
        files
            .iter()
            .any(|f| f == "node_modules/@gaia-test/tiny/index.js")
    );
}

fn list(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .expect("read")
        .map(|e| e.expect("entry"))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if entry.file_type().expect("type").is_dir() {
            list(root, &entry.path(), out);
        } else {
            out.push(
                entry
                    .path()
                    .strip_prefix(root)
                    .expect("in")
                    .display()
                    .to_string(),
            );
        }
    }
}

#[test]
fn a_dependency_missing_from_the_cache_fails_offline() {
    let Some(images) = images(NODE_IMAGE, NODE_DIGEST, "usr/local") else {
        return;
    };
    let home = images.toolchain.clone();
    let dir = tempfile::tempdir().expect("tempdir");
    let (app, _) = inputs(&home, dir.path());
    let empty = dir.path().join("empty-cache");
    std::fs::create_dir_all(&empty).expect("mkdir");
    let store = dir.path().join("store");
    let error = build_node(
        &store.display().to_string(),
        BWRAP,
        stored(&store, &images.root),
        &home.display().to_string(),
        stored(&store, &app),
        stored(&store, &empty),
        "",
        &Ctx::new(),
    )
    .expect_err("offline, nothing cached");
    let text = format!("{error}\n{}", error.stderr_tail().join("\n"));
    println!("{text}");
    assert!(
        text.contains("ENOTCACHED") || text.contains("offline"),
        "{text}"
    );
}
