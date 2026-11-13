//! A small workspace, built twice in two fresh stores: the same bytes.
//!
//! The workspace has a binary, a library it uses by path, and a crates.io
//! dependency taken from a vendored tree, so the build is offline. The image
//! is `debian:trixie-slim`, pinned by its index digest and pulled by the OCI
//! source node (this needs the network). The toolchain is this host's
//! sysroot, bound read-only. Where bubblewrap or user namespaces are missing
//! the tests say SKIPPED, unless `GAIA_REQUIRE_SANDBOX=1`, when they fail.
//! Where the pull or the toolchain's std for the target is missing they say
//! SKIPPED, unless `GAIA_REQUIRE_TOOLS=1`, when they fail.

use gaia_build_rust::build_rust;
use gaia_source_oci::source;
use gaia_step::Ctx;
use gaia_store::{Store, file_digest};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::path::{Path, PathBuf};

const BWRAP: &str = "/usr/bin/bwrap";
const CURL: &str = "/usr/bin/curl";
/// The image: the index of `debian:trixie-slim`, pinned, for linux/amd64.
const IMAGE: &str = "docker.io/library/debian:trixie-slim";
const IMAGE_DIGEST: &str =
    "sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2";
const TRIPLE: &str = "aarch64-unknown-linux-musl";

/// The checksum the lock and the vendored crate agree on.
const TINY_SUM: &str = "5f2c9a1e0b7d4c3e8a6f1b2d9c0e7a4b3c6d8e1f2a5b7c9d0e3f6a8b1c4d7e0f";

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

/// The image, pulled into `store` by digest. Missing the network, a skip
/// unless required.
fn pull(store: &Path) -> Option<Tree> {
    let store_text = store.display().to_string();
    match source(
        &store_text,
        CURL,
        IMAGE,
        IMAGE_DIGEST,
        "linux/amd64",
        "",
        &Ctx::new(),
    ) {
        Ok((tree, _)) => Some(tree),
        Err(error) => {
            assert!(
                !required("GAIA_REQUIRE_TOOLS"),
                "GAIA_REQUIRE_TOOLS is set, and the image did not pull: {error}"
            );
            println!("SKIPPED: the image did not pull: {error}");
            None
        }
    }
}

fn stored_tree(store: &Path, dir: &Path) -> Tree {
    let store = Store::open(store).expect("store");
    Tree {
        object: store.put_tree(dir).expect("put").into(),
        entries: Vec::new(),
    }
}

/// The artifacts, and each file of the output tree: path, size and digest.
type Built = (Vec<Artifact>, Vec<(String, u64, String)>);

/// Builds `app` in a fresh store; gives its artifacts and their files.
/// `None` when the image cannot be pulled here.
fn build(root: &Path, triple: &str, cpu: &str, features: &[&str]) -> Option<Built> {
    let src = root.join("src");
    let vendor = root.join("vendor");
    workspace(&src);
    vendored(&vendor);
    let store = root.join("store");
    let image = pull(&store)?;
    let toolchain = toolchain();
    let (tree, artifacts) = build_rust(
        &store.display().to_string(),
        BWRAP,
        image,
        &toolchain.display().to_string(),
        stored_tree(&store, &src),
        stored_tree(&store, &vendor),
        "app",
        Some(&features.join(",")),
        None,
        triple,
        Some(&cpu.to_string()),
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
    Some((artifacts, files))
}

/// Whether the toolchain can build `triple` here: a skip unless required.
fn target_here(triple: &str) -> bool {
    let toolchain = toolchain();
    let ok = has_target(&toolchain, triple);
    if !ok {
        assert!(
            !required("GAIA_REQUIRE_TOOLS"),
            "GAIA_REQUIRE_TOOLS is set, and the toolchain has no std for {triple}"
        );
        println!(
            "SKIPPED: the toolchain at {} has no std for {triple}",
            toolchain.display()
        );
    }
    ok
}

#[test]
fn aarch64_musl_twice_gives_identical_digests() {
    if !sandbox_here() || !target_here(TRIPLE) {
        return;
    }
    let (a, b) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let Some((artifacts, first)) = build(a.path(), TRIPLE, "cortex-a55", &[]) else {
        return;
    };
    let (_, second) = build(b.path(), TRIPLE, "cortex-a55", &[]).expect("pulls again");
    println!("{TRIPLE} cortex-a55 (sandboxed, image {IMAGE_DIGEST}):");
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
    let home = std::env::var("HOME").unwrap_or_default();
    assert!(
        home.is_empty() || !bytes.windows(home.len()).any(|w| w == home.as_bytes()),
        "{home} is in the binary"
    );
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].0, "bin/app");
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].kind, ArtifactKind::Binary);

    // A feature is another build.
    let c = tempfile::tempdir().expect("tempdir");
    let (_, loud) = build(c.path(), TRIPLE, "cortex-a55", &["loud"]).expect("pulls");
    assert_ne!(loud[0].2, first[0].2);

    // The run logs: every tool ran under bubblewrap, and cargo did.
    let mut tools = Vec::new();
    for dir in std::fs::read_dir(a.path().join("logs")).expect("logs") {
        let dir = dir.expect("entry").path();
        // Only this builder's steps: the image pull is the OCI source node's.
        if !dir.is_dir()
            || !dir
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("gaia.build.rust"))
        {
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
    println!("run log tools:");
    for tool in &tools {
        println!("  {tool}");
    }
    assert!(
        tools.iter().all(|t| t.starts_with(BWRAP)),
        "a tool ran outside the sandbox: {tools:?}"
    );
    assert!(
        tools
            .iter()
            .any(|t| t.contains("/toolchain/bin/cargo build")),
        "no cargo build in the run logs: {tools:?}"
    );
}

/// A source like a real repository's: a crates.io dependency named only in
/// `Cargo.lock`, its own profile, and its own flags for the target in
/// `.cargo/config.toml`. Without the config's flag the program does not
/// compile, so a build that replaces the source's flags fails here.
fn repository(dir: &Path) {
    write(
        &dir.join("Cargo.toml"),
        "[package]\nname = \"svc\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ncfg-if = \"=1.0.0\"\n\n[profile.service]\ninherits = \"release\"\nopt-level = \"s\"\n",
    );
    write(
        &dir.join("src/main.rs"),
        "#[cfg(not(from_the_sources_config))]\ncompile_error!(\"the source's own rustflags were dropped\");\n\nfn main() {\n    cfg_if::cfg_if! {\n        if #[cfg(unix)] { println!(\"unix\"); } else { println!(\"other\"); }\n    }\n}\n",
    );
    write(
        &dir.join(".cargo/config.toml"),
        &format!("[target.{TRIPLE}]\nrustflags = [\"--cfg\", \"from_the_sources_config\"]\n"),
    );
}

/// `Cargo.lock` for `dir`, written by the host's cargo. Missing the
/// network, a skip unless required.
fn lock(dir: &Path) -> bool {
    let out = std::process::Command::new(toolchain().join("bin/cargo"))
        .args(["generate-lockfile", "--manifest-path"])
        .arg(dir.join("Cargo.toml"))
        .output()
        .expect("cargo");
    if !out.status.success() {
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(
            !required("GAIA_REQUIRE_TOOLS"),
            "GAIA_REQUIRE_TOOLS is set, and the lock was not made: {error}"
        );
        println!("SKIPPED: no Cargo.lock without the network: {error}");
    }
    out.status.success()
}

#[test]
fn a_repository_is_vendored_from_its_lock_and_built_with_its_profile_and_flags() {
    if !sandbox_here() || !target_here(TRIPLE) {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src");
    repository(&src);
    if !lock(&src) {
        return;
    }
    let store = dir.path().join("store");
    let Some(image) = pull(&store) else {
        return;
    };
    let store_text = store.display().to_string();
    let toolchain = toolchain().display().to_string();
    let source = stored_tree(&store, &src);

    let vendor = gaia_build_rust::vendor(&store_text, &toolchain, source.clone(), &Ctx::new())
        .expect("vendors");
    let opened = Store::open(&store).expect("store");
    let vendor_dir = vendor.object.path_in(&opened).expect("in store");
    let mut crates: Vec<String> = std::fs::read_dir(&vendor_dir)
        .expect("read")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    crates.sort();
    println!("vendored: {crates:?}");
    assert_eq!(crates, [".cargo-config.toml", "cfg-if-1.0.0"]);
    let config = std::fs::read_to_string(vendor_dir.join(".cargo-config.toml")).expect("config");
    println!("{config}");
    assert!(config.contains("directory = \"@vendor@\""), "{config}");

    // The same lock is found again: nothing is fetched twice.
    let again = gaia_build_rust::vendor(&store_text, &toolchain, source.clone(), &Ctx::new())
        .expect("again");
    assert_eq!(again.object, vendor.object);

    // Offline, with the source's profile, and the source's flags kept.
    let (tree, artifacts) = build_rust(
        &store_text,
        BWRAP,
        image,
        &toolchain,
        source,
        vendor,
        "svc",
        None,
        Some(&"service".to_string()),
        TRIPLE,
        None,
        &Ctx::new(),
    )
    .expect("builds with the service profile and the config's flag");
    let out = tree.object.path_in(&opened).expect("in store");
    let size = std::fs::metadata(out.join("bin/svc"))
        .expect("the binary")
        .len();
    println!("bin/svc: {size} bytes ({} artifacts)", artifacts.len());
    assert!(size > 0);
}

#[test]
fn a_source_without_a_lock_is_refused_by_the_vendor_step() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src");
    write(
        &src.join("Cargo.toml"),
        "[package]\nname = \"svc\"\nversion = \"0.1.0\"\n",
    );
    let store = dir.path().join("store");
    let source = stored_tree(&store, &src);
    let error = gaia_build_rust::vendor(
        &store.display().to_string(),
        &toolchain().display().to_string(),
        source,
        &Ctx::new(),
    )
    .expect_err("no lock");
    assert!(error.to_string().contains("has no Cargo.lock"), "{error}");
}
