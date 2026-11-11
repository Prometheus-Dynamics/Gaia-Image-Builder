//! A small project with a console script and a dependency from a wheel,
//! built twice in two stores: the same digests.

use gaia_build_python::build_python;
use gaia_step::Ctx;
use gaia_store::{Store, file_digest, tree_digest};
use gaia_types::Tree;
use std::path::Path;
use std::process::Command;

const PYTHON: &str = "/usr/bin/python3";

/// Whether the interpreter can build a wheel offline: pip, and setuptools
/// new enough to build wheels without the `wheel` package.
fn available() -> bool {
    let ok = Path::new(PYTHON).exists()
        && Command::new(PYTHON)
            .args(["-c", "import pip, setuptools, sys; v = tuple(int(x) for x in setuptools.__version__.split('.')[:2]); sys.exit(0 if v >= (70, 1) else 1)"])
            .status()
            .is_ok_and(|s| s.success());
    if !ok {
        println!("SKIPPED: {PYTHON} has no pip with setuptools >= 70.1");
    }
    ok
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, text).expect("write");
}

fn project(dir: &Path) {
    write(
        &dir.join("pyproject.toml"),
        "[build-system]\nrequires = [\"setuptools>=70.1\"]\nbuild-backend = \"setuptools.build_meta\"\n\n[project]\nname = \"hello\"\nversion = \"1.0\"\ndependencies = [\"tinydep\"]\n\n[project.scripts]\nhello = \"hello:main\"\n",
    );
    write(
        &dir.join("hello/__init__.py"),
        "import tinydep\n\ndef main():\n    print('hello', tinydep.ANSWER)\n",
    );
}

/// A dependency wheel, as the lock's wheel tree holds it.
fn dependency(dir: &Path) {
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
    let status = Command::new(PYTHON)
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
fn build(root: &Path) -> (String, String, u64, std::path::PathBuf) {
    let (src, deps, store) = (root.join("src"), root.join("wheels"), root.join("store"));
    project(&src);
    dependency(&deps);
    let (tree, artifacts) = build_python(
        &store.display().to_string(),
        PYTHON,
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
    if !available() {
        return;
    }
    let (a, b) = (
        tempfile::tempdir().expect("tempdir"),
        tempfile::tempdir().expect("tempdir"),
    );
    let first = build(a.path());
    let second = build(b.path());
    println!("tree   {}", first.0);
    println!("wheel  {} ({} bytes)", first.1, first.2);
    assert_eq!(
        (&first.0, &first.1),
        (&second.0, &second.1),
        "two stores, the same digests"
    );

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
    let out = Command::new(PYTHON)
        .arg(first.3.join("site/bin/hello"))
        .env("PYTHONPATH", first.3.join("site"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
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
