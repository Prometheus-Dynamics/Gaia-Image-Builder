//! A small module with a vendored dependency, built for aarch64 twice in
//! two stores: the same digests.

use gaia_build_go::{build_go, go_target};
use gaia_step::Ctx;
use gaia_store::{Store, file_digest};
use gaia_types::Tree;
use std::path::{Path, PathBuf};
use std::process::Command;

fn goroot() -> Option<PathBuf> {
    let out = Command::new("go")
        .args(["env", "GOROOT"])
        .env("GOTOOLCHAIN", "local")
        .output()
        .ok()?;
    let root = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim());
    if root.join("bin/go").is_file() {
        Some(root)
    } else {
        println!("SKIPPED: no Go toolchain here");
        None
    }
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

fn build(root: &Path, goroot: &Path, triple: &str) -> Vec<(String, u64, String)> {
    let (src, modules, store) = (root.join("src"), root.join("modules"), root.join("store"));
    module(&src);
    std::fs::create_dir_all(&modules).expect("mkdir");
    let (tree, artifacts) = build_go(
        &store.display().to_string(),
        &goroot.display().to_string(),
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
    files
}

#[test]
fn a_small_module_builds_for_aarch64_twice_to_the_same_digest() {
    let Some(goroot) = goroot() else { return };
    let (a, b) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let triple = "aarch64-unknown-linux-musl";
    let first = build(a.path(), &goroot, triple);
    let second = build(b.path(), &goroot, triple);
    println!("{triple} (linux/arm64):");
    for (path, size, digest) in &first {
        println!("  {path:<10} {size:>8} bytes  {digest}");
    }
    assert_eq!(first, second, "two stores, the same bytes");
    let names: Vec<&str> = first.iter().map(|f| f.0.as_str()).collect();
    assert_eq!(names, ["bin/bye", "bin/hello"]);
    // Each is an aarch64 ELF (e_machine 183).
    let mut machines = Vec::new();
    walk(&a.path().join("store"), &mut |p| {
        if p.parent().is_some_and(|d| d.ends_with("bin")) {
            let bytes = std::fs::read(p).expect("read");
            machines.push(u16::from_le_bytes([bytes[18], bytes[19]]));
        }
    });
    assert!(
        !machines.is_empty() && machines.iter().all(|m| *m == 183),
        "{machines:?}"
    );
}

#[test]
fn the_host_build_runs() {
    let Some(goroot) = goroot() else { return };
    let dir = tempfile::tempdir().expect("tempdir");
    build(dir.path(), &goroot, "x86_64-unknown-linux-gnu");
    // The built tree is under the store; find hello by walking it.
    let mut found = None;
    walk(&dir.path().join("store"), &mut |p| {
        if p.file_name().is_some_and(|n| n == "hello")
            && p.parent().is_some_and(|d| d.ends_with("bin"))
        {
            found = Some(p.to_path_buf());
        }
    });
    let out = Command::new(found.expect("hello")).output().expect("runs");
    let text = String::from_utf8_lossy(&out.stdout);
    println!("runs: {}", text.trim());
    assert_eq!(text.trim(), "hello 42");
}

fn walk(dir: &Path, f: &mut dyn FnMut(&Path)) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            walk(&p, f);
        } else {
            f(&p);
        }
    }
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
