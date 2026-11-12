//! A small app with one locked dependency from an npm cache, built twice in
//! two stores: the same bundle and package, and the bundle runs.

use gaia_build_node::build_node;
use gaia_step::Ctx;
use gaia_store::{Store, file_digest, tree_digest};
use gaia_types::Tree;
use std::path::{Path, PathBuf};
use std::process::Command;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

/// The Node.js install whose `node` and `npm` are on `PATH`.
fn node_home() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let node = std::env::split_paths(&path)
        .map(|d| d.join("node"))
        .find(|p| p.is_file())?;
    let home = node.parent()?.parent()?.to_path_buf();
    home.join("bin/npm").exists().then_some(home)
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
    let Some(home) = node_home() else {
        assert!(
            std::env::var_os("GAIA_REQUIRE_TOOLS").is_none(),
            "GAIA_REQUIRE_TOOLS is set, and node and npm are missing"
        );
        println!("SKIPPED: no node and npm here");
        return;
    };
    let shared = tempfile::tempdir().expect("tempdir");
    let (app, cache) = inputs(&home, shared.path());
    let mut results = Vec::new();
    for _ in 0..2 {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = dir.path().join("store");
        let (bundle, package) = build_node(
            &store.display().to_string(),
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
        drop(dir);
    }
    let (bundle, package, size, ran, files) = &results[0];
    println!("node {}", home.display());
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
    let Some(home) = node_home() else {
        assert!(
            std::env::var_os("GAIA_REQUIRE_TOOLS").is_none(),
            "GAIA_REQUIRE_TOOLS is set, and node and npm are missing"
        );
        println!("SKIPPED: no node and npm here");
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let (app, _) = inputs(&home, dir.path());
    let empty = dir.path().join("empty-cache");
    std::fs::create_dir_all(&empty).expect("mkdir");
    let store = dir.path().join("store");
    let error = build_node(
        &store.display().to_string(),
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
