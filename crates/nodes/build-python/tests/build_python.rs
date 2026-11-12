//! A small project with a console script and a dependency from a wheel,
//! built twice in two stores: the same digests. The installed RECORDs match
//! their files, and `pip check` passes on the installed tree.
//!
//! Without Python and pip the tests say SKIPPED, unless `GAIA_REQUIRE_TOOLS=1`,
//! when they fail.

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
        assert!(
            std::env::var_os("GAIA_REQUIRE_TOOLS").is_none(),
            "GAIA_REQUIRE_TOOLS is set, and {PYTHON} has no pip with setuptools >= 70.1"
        );
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

/// Every RECORD row that has a hash names a file whose sha256 (urlsafe
/// base64, no padding) and size are the ones RECORD gives. Python's own
/// hashlib checks them, not the code under test.
#[test]
fn installed_records_match_their_files() {
    if !available() {
        return;
    }
    let root = tempfile::tempdir().expect("tempdir");
    let (_, _, _, out) = build(root.path());
    let site = out.join("site");

    let check = r#"
import base64, csv, glob, hashlib, os, sys
site = sys.argv[1]
bad, checked = [], []
for info in sorted(glob.glob(os.path.join(site, '*.dist-info'))):
    with open(os.path.join(info, 'RECORD'), newline='') as f:
        for path, digest, size in csv.reader(f):
            if not digest:
                continue
            # pip writes a target's scripts as '../../bin/x'; the file is bin/x.
            rel = path
            while rel.startswith('../'):
                rel = rel[3:]
            data = open(os.path.join(site, rel), 'rb').read()
            want = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b'=').decode()
            checked.append(path)
            if digest != 'sha256=' + want or int(size) != len(data):
                bad.append(path)
for path in checked:
    print('checked', path)
for path in bad:
    print('stale', path)
sys.exit(1 if bad or not checked else 0)
"#;
    let run = Command::new(PYTHON)
        .arg("-c")
        .arg(check)
        .arg(&site)
        .output()
        .expect("python");
    let text = String::from_utf8_lossy(&run.stdout);
    print!("{text}");
    assert!(run.status.success(), "RECORD does not match the files");
    assert!(text.contains("bin/hello"), "the script is checked: {text}");

    // A fresh venv (its own pip, no system packages) with the installed tree
    // on its path: pip check finds nothing broken in the tree.
    let venv = root.path().join("venv");
    let made = Command::new(PYTHON)
        .args(["-m", "venv"])
        .arg(&venv)
        .output()
        .expect("venv");
    assert!(made.status.success(), "venv: {made:?}");
    let pip = Command::new(venv.join("bin/python"))
        .args(["-m", "pip", "check"])
        .env("PYTHONPATH", &site)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PIP_NO_INPUT", "1")
        .env("PIP_NO_CACHE_DIR", "1")
        .env("PIP_CONFIG_FILE", "/dev/null")
        .env("PIP_DISABLE_PIP_VERSION_CHECK", "1")
        .output()
        .expect("pip");
    let report = String::from_utf8_lossy(&pip.stdout);
    print!("pip check: {report}");
    assert!(pip.status.success(), "pip check failed: {report}");
    assert!(report.contains("No broken requirements found"), "{report}");
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
