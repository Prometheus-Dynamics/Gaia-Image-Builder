//! A small project with a console script and a dependency from a wheel,
//! built twice in two fresh stores: the same digests.
//!
//! The root is `debian:trixie-slim` and Python is the install in
//! `python:3.13-slim-trixie` (`/usr/local`), both pinned by their index
//! digests and pulled by the OCI source node (this needs the network).
//! That Python has pip but no setuptools, so the project's build backend
//! is its own, in the tree (`backend-path`). The dependency wheel is made
//! with the same Python, run here. Where bubblewrap or user namespaces are
//! missing the tests say SKIPPED, unless `GAIA_REQUIRE_SANDBOX=1`, when
//! they fail. Where a pull is missing they say SKIPPED, unless
//! `GAIA_REQUIRE_TOOLS=1`, when they fail.

use gaia_build_python::build_python;
use gaia_step::{Ctx, logs_dir};
use gaia_store::{Store, file_digest, tree_digest};
use gaia_types::Tree;
use std::path::{Path, PathBuf};
use std::process::Command;

const PYTHON_IMAGE: &str = "docker.io/library/python:3.13-slim-trixie";
const PYTHON_DIGEST: &str =
    "sha256:e48f59cdba69a3cbd9ed2cf0f348d90d45a8d350d98134cf1070bc106ee9ed90";

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

/// The toolchain's Python, run here, with its own libraries.
fn python(images: &Images) -> Command {
    let mut command = Command::new(images.toolchain.join("bin/python3"));
    command
        .env_clear()
        .env("LD_LIBRARY_PATH", images.toolchain.join("lib"))
        .env("PYTHONDONTWRITEBYTECODE", "1");
    command
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

fn project(dir: &Path) {
    write(
        &dir.join("pyproject.toml"),
        "[build-system]\nrequires = []\nbuild-backend = \"backend\"\nbackend-path = [\"_backend\"]\n",
    );
    write(&dir.join("_backend/backend.py"), BACKEND);
    write(
        &dir.join("hello/__init__.py"),
        "import tinydep\n\ndef main():\n    print('hello', tinydep.ANSWER)\n",
    );
}

/// The project's build backend: a wheel of `hello`, with its metadata and
/// console script, at a fixed time.
const BACKEND: &str = r#"
import base64, hashlib, zipfile

def build_wheel(wheel_directory, config_settings=None, metadata_directory=None):
    info = 'hello-1.0.dist-info/'
    with open('hello/__init__.py', 'rb') as f:
        files = {'hello/__init__.py': f.read()}
    files[info + 'METADATA'] = b'Metadata-Version: 2.1\nName: hello\nVersion: 1.0\nRequires-Dist: tinydep\n'
    files[info + 'WHEEL'] = b'Wheel-Version: 1.0\nGenerator: test\nRoot-Is-Purelib: true\nTag: py3-none-any\n'
    files[info + 'entry_points.txt'] = b'[console_scripts]\nhello = hello:main\n'
    def digest(data):
        return 'sha256=' + base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b'=').decode()
    record = ''.join(f'{name},{digest(data)},{len(data)}\n' for name, data in files.items())
    files[info + 'RECORD'] = (record + info + 'RECORD,,\n').encode()
    name = 'hello-1.0-py3-none-any.whl'
    with zipfile.ZipFile(wheel_directory + '/' + name, 'w') as z:
        for path, data in files.items():
            z.writestr(zipfile.ZipInfo(path, (1980, 1, 1, 0, 0, 0)), data)
    return name
"#;

/// A dependency wheel, as the lock's wheel tree holds it.
fn dependency(images: &Images, dir: &Path) {
    std::fs::create_dir_all(dir).expect("mkdir");
    let script = r#"
import sys, zipfile
info = 'tinydep-1.0.dist-info/'
files = {
    'tinydep/__init__.py': 'ANSWER = 42\n',
    info + 'METADATA': 'Metadata-Version: 2.1\nName: tinydep\nVersion: 1.0\n',
    info + 'WHEEL': 'Wheel-Version: 1.0\nGenerator: test\nRoot-Is-Purelib: true\nTag: py3-none-any\n',
}
record = ''.join(f'{name},,\n' for name in files) + info + 'RECORD,,\n'
with zipfile.ZipFile(sys.argv[1] + '/tinydep-1.0-py3-none-any.whl', 'w') as z:
    for name, text in list(files.items()) + [(info + 'RECORD', record)]:
        z.writestr(zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0)), text)
"#;
    let status = python(images)
        .arg("-c")
        .arg(script)
        .arg(dir)
        .status()
        .expect("python");
    assert!(status.success());
}

fn stored(store: &Path, dir: &Path) -> Tree {
    let store = Store::open(store).expect("store");
    Tree {
        object: store.put_tree(dir).expect("put").into(),
        entries: Vec::new(),
    }
}

/// Builds in a fresh store under `root`: the output tree's digest, the
/// wheel's digest and size, and the output directory.
fn build(images: &Images, root: &Path) -> (String, String, u64, PathBuf) {
    let (src, deps, store) = (root.join("src"), root.join("wheels"), root.join("store"));
    project(&src);
    dependency(images, &deps);
    let (tree, artifacts) = build_python(
        &store.display().to_string(),
        BWRAP,
        stored(&store, &images.root),
        &images.toolchain.display().to_string(),
        stored(&store, &src),
        stored(&store, &deps),
        "",
        "/usr/bin/python3",
        &Ctx::new(),
    )
    .expect("builds");
    let s = Store::open(&store).expect("store");
    let out = tree.object.path_in(&s).expect("in");
    let wheel = artifacts[0].object.path_in(&s).expect("in");
    (
        tree_digest(&out).expect("digest").to_string(),
        file_digest(&wheel).expect("digest").to_string(),
        std::fs::metadata(&wheel).expect("size").len(),
        out,
    )
}

#[test]
fn a_small_package_builds_offline_twice_to_the_same_digest() {
    let Some(images) = images(PYTHON_IMAGE, PYTHON_DIGEST, "usr/local") else {
        return;
    };
    let (a, b) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let first = build(&images, a.path());
    let second = build(&images, b.path());
    println!("{PYTHON_IMAGE} (sandboxed, root {IMAGE_DIGEST})");
    println!("tree   {}", first.0);
    println!("wheel  {} ({} bytes)", first.1, first.2);
    assert_eq!(
        (&first.0, &first.1),
        (&second.0, &second.1),
        "two stores, the same digests"
    );
    let store = Store::open(a.path().join("store")).expect("store");
    all_sandboxed(&logs_dir(&store), "/toolchain/bin/python3 -m pip wheel");

    let mut listed = Vec::new();
    list(&first.3, &first.3, &mut listed);
    for path in &listed {
        println!("  {path}");
    }
    assert!(
        listed
            .iter()
            .any(|p| p == "wheels/hello-1.0-py3-none-any.whl")
    );
    assert!(listed.iter().any(|p| p == "site/tinydep/__init__.py"));
    assert!(!listed.iter().any(|p| p.ends_with(".pyc")), "no bytecode");
    let script = std::fs::read_to_string(first.3.join("site/bin/hello")).expect("script");
    assert!(script.starts_with("#!/usr/bin/python3\n"), "{script}");

    // The installed tree runs.
    let out = python(&images)
        .arg(first.3.join("site/bin/hello"))
        .env("PYTHONPATH", first.3.join("site"))
        .output()
        .expect("runs");
    let text = String::from_utf8_lossy(&out.stdout);
    println!("runs: {}", text.trim());
    assert_eq!(text.trim(), "hello 42");
}

fn list(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).expect("read").flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if entry.file_type().expect("type").is_dir() {
            list(root, &path, out);
        } else {
            out.push(path.strip_prefix(root).expect("in").display().to_string());
        }
    }
}
