//! A small workspace, built twice in two stores: the same bytes.
//!
//! The workspace has a binary, a library it uses by path, and a crates.io
//! dependency taken from a vendored tree, so the build is offline.

use gaia_build_rust::build_rust;
use gaia_step::Ctx;
use gaia_store::{Store, file_digest};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::path::{Path, PathBuf};

/// The checksum the lock and the vendored crate agree on.
const TINY_SUM: &str = "5f2c9a1e0b7d4c3e8a6f1b2d9c0e7a4b3c6d8e1f2a5b7c9d0e3f6a8b1c4d7e0f";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

fn workspace(dir: &Path) {
    write(
        &dir.join("Cargo.toml"),
        "[workspace]\nmembers = [\"app\", \"shout\"]\nresolver = \"2\"\n\n[profile.release]\nopt-level = \"s\"\npanic = \"abort\"\n",
    );
    write(
        &dir.join("shout/Cargo.toml"),
        "[package]\nname = \"shout\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(
        &dir.join("shout/src/lib.rs"),
        "pub fn shout(s: &str) -> String { s.to_uppercase() + \"!\" }\n",
    );
    write(
        &dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nshout = { path = \"../shout\" }\ntiny = \"1\"\n\n[features]\nloud = []\n",
    );
    write(
        &dir.join("app/src/main.rs"),
        "fn main() {\n    let word = if cfg!(feature = \"loud\") { \"loud\" } else { \"quiet\" };\n    println!(\"{} {}\", shout::shout(word), tiny::answer());\n}\n",
    );
    write(
        &dir.join("Cargo.lock"),
        &format!(
            "version = 4\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\n \"shout\",\n \"tiny\",\n]\n\n[[package]]\nname = \"shout\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"tiny\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"{TINY_SUM}\"\n"
        ),
    );
}

/// What `cargo vendor` would write for `tiny`.
fn vendored(dir: &Path) {
    write(
        &dir.join("tiny/Cargo.toml"),
        "[package]\nname = \"tiny\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    );
    write(
        &dir.join("tiny/src/lib.rs"),
        "pub fn answer() -> u32 { 42 }\n",
    );
    write(
        &dir.join("tiny/.cargo-checksum.json"),
        &format!("{{\"files\":{{}},\"package\":\"{TINY_SUM}\"}}"),
    );
}

fn toolchain() -> PathBuf {
    let out = std::process::Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .expect("rustc");
    PathBuf::from(String::from_utf8(out.stdout).expect("utf8").trim())
}

fn has_target(toolchain: &Path, triple: &str) -> bool {
    toolchain.join("lib/rustlib").join(triple).is_dir()
}

fn stored_tree(store: &Path, dir: &Path) -> Tree {
    let store = Store::open(store).expect("store");
    Tree {
        object: store.put_tree(dir).expect("put").into(),
        entries: Vec::new(),
    }
}

/// Builds `app` in a fresh store; gives its artifacts and their files.
fn build(
    root: &Path,
    triple: &str,
    cpu: &str,
    features: &[&str],
) -> (Vec<Artifact>, Vec<(String, u64, String)>) {
    let src = root.join("src");
    let vendor = root.join("vendor");
    workspace(&src);
    vendored(&vendor);
    let store = root.join("store");
    let (tree, artifacts) = build_rust(
        &store.display().to_string(),
        &toolchain().display().to_string(),
        stored_tree(&store, &src),
        stored_tree(&store, &vendor),
        "app",
        &features.join(","),
        triple,
        cpu,
        &Ctx::new(),
    )
    .expect("builds");
    let store = Store::open(&store).expect("store");
    let out = tree.object.path_in(&store).expect("in store");
    let mut files = Vec::new();
    for sub in ["bin", "lib"] {
        for entry in std::fs::read_dir(out.join(sub))
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            files.push((
                format!("{sub}/{}", entry.file_name().to_string_lossy()),
                std::fs::metadata(&path).expect("size").len(),
                file_digest(&path).expect("digest").to_string(),
            ));
        }
    }
    files.sort();
    (artifacts, files)
}

fn twice(triple: &str, cpu: &str) {
    let toolchain = toolchain();
    if !has_target(&toolchain, triple) {
        assert!(
            std::env::var_os("GAIA_REQUIRE_TOOLS").is_none(),
            "GAIA_REQUIRE_TOOLS is set, and the toolchain has no std for a target the test builds"
        );
        println!(
            "SKIPPED: the toolchain at {} has no std for {triple}",
            toolchain.display()
        );
        return;
    }
    let (a, b) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let (artifacts, first) = build(a.path(), triple, cpu, &[]);
    let (_, second) = build(b.path(), triple, cpu, &[]);
    println!(
        "{triple} {}:",
        if cpu.is_empty() { "(generic)" } else { cpu }
    );
    for (path, size, digest) in &first {
        println!("  {path:<12} {size:>8} bytes  {digest}");
    }
    assert_eq!(first, second, "two stores, the same bytes");
    // No path of the build host is in the binary.
    let store = Store::open(a.path().join("store")).expect("store");
    let bytes = std::fs::read(artifacts[0].object.path_in(&store).expect("in")).expect("read");
    let root = a.path().display().to_string();
    assert!(
        !bytes.windows(root.len()).any(|w| w == root.as_bytes()),
        "{root} is in the binary"
    );
    for home in [
        std::env::var("HOME").unwrap_or_default(),
        toolchain.display().to_string(),
    ] {
        assert!(
            home.is_empty() || !bytes.windows(home.len()).any(|w| w == home.as_bytes()),
            "{home} is in the binary"
        );
    }
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].0, "bin/app");
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].kind, ArtifactKind::Binary);

    // A feature is another build.
    let c = tempfile::tempdir().expect("tempdir");
    let (_, loud) = build(c.path(), triple, cpu, &["loud"]);
    assert_ne!(loud[0].2, first[0].2);
}

#[test]
fn aarch64_musl_twice_gives_identical_digests() {
    twice("aarch64-unknown-linux-musl", "cortex-a55");
}

#[test]
fn the_host_target_twice_gives_identical_digests_and_runs() {
    let host = "x86_64-unknown-linux-gnu";
    twice(host, "");
    if !has_target(&toolchain(), host) {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let (artifacts, _) = build(dir.path(), host, "", &["loud"]);
    let store = Store::open(dir.path().join("store")).expect("store");
    let bin = artifacts[0].object.path_in(&store).expect("in store");
    let out = std::process::Command::new(&bin).output().expect("runs");
    let text = String::from_utf8_lossy(&out.stdout);
    println!("runs: {}", text.trim());
    assert_eq!(text.trim(), "LOUD! 42");
}
