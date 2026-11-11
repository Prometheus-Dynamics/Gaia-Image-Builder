//! Strip and runtime closure on real ELF files, built with the host's C
//! compiler. The report names the unused library and nothing else as
//! unused; the pruned tree lacks it; a stripped program still runs under
//! qemu-user. A tool that is missing says SKIPPED, unless
//! `GAIA_REQUIRE_TOOLS=1`, when it fails: the C compiler (cc), strip, and
//! the qemu-user binary for this host's architecture.

use gaia_step::{Ctx, Store};
use gaia_strip::{prune_libs, strip_elf, unused_libs};
use gaia_types::{Artifact, FileEntry, Tree};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The first of `names` found in the usual directories.
fn find(names: &[&str]) -> Option<String> {
    for name in names {
        for dir in ["/usr/bin", "/usr/local/bin", "/bin"] {
            let path = format!("{dir}/{name}");
            if Path::new(&path).exists() {
                return Some(path);
            }
        }
    }
    None
}

/// A tool, or `None` (SKIPPED) when it is missing and not required.
fn tool(names: &[&str], what: &str) -> Option<String> {
    let found = find(names);
    if found.is_none() {
        assert!(
            std::env::var_os("GAIA_REQUIRE_TOOLS").is_none(),
            "GAIA_REQUIRE_TOOLS is set, and {what} is missing"
        );
        println!("SKIPPED: {what} is not installed");
    }
    found
}

/// The qemu-user binary for this host's architecture.
fn qemu() -> Option<String> {
    let arch = std::env::consts::ARCH;
    tool(
        &[&format!("qemu-{arch}-static"), &format!("qemu-{arch}")],
        "qemu-user",
    )
}

fn run(program: &str, args: &[&str]) {
    let status = Command::new(program)
        .args(args)
        .status()
        .unwrap_or_else(|e| panic!("{program}: {e}"));
    assert!(status.success(), "{program} {args:?} failed");
}

/// A tree under `root` with a program, a library it needs, a library nothing
/// needs, and a symlink to each library. The program prints `app 7`.
fn fixture(cc: &str, root: &Path) -> (String, Tree) {
    let src = root.join("src");
    std::fs::create_dir_all(&src).expect("mkdir");
    std::fs::write(src.join("used.c"), "int used_fn(void) { return 7; }\n").expect("write");
    std::fs::write(src.join("unused.c"), "int unused_fn(void) { return 9; }\n").expect("write");
    std::fs::write(
        src.join("app.c"),
        "#include <stdio.h>\nint used_fn(void);\nint main(void) { printf(\"app %d\\n\", used_fn()); return 0; }\n",
    )
    .expect("write");
    let tree = root.join("tree");
    std::fs::create_dir_all(tree.join("usr/bin")).expect("mkdir");
    std::fs::create_dir_all(tree.join("usr/lib")).expect("mkdir");
    let lib = tree.join("usr/lib");
    let lib_str = lib.display().to_string();
    run(
        cc,
        &[
            "-shared",
            "-fPIC",
            "-g",
            "-Wl,-soname,libused.so.1",
            "-o",
            &format!("{lib_str}/libused.so.1"),
            &src.join("used.c").display().to_string(),
        ],
    );
    run(
        cc,
        &[
            "-shared",
            "-fPIC",
            "-g",
            "-Wl,-soname,libunused.so.1",
            "-o",
            &format!("{lib_str}/libunused.so.1"),
            &src.join("unused.c").display().to_string(),
        ],
    );
    std::os::unix::fs::symlink("libused.so.1", lib.join("libused.so")).expect("link");
    std::os::unix::fs::symlink("libunused.so.1", lib.join("libunused.so")).expect("link");
    run(
        cc,
        &[
            "-g",
            "-o",
            &tree.join("usr/bin/app").display().to_string(),
            &src.join("app.c").display().to_string(),
            &format!("-L{lib_str}"),
            "-lused",
        ],
    );
    let store_path = root.join("store");
    let store = Store::open(&store_path).expect("store");
    let tree = Tree {
        object: store.put_tree(&tree).expect("put").into(),
        entries: Vec::new(),
    };
    (store_path.display().to_string(), tree)
}

fn placed(store: &str, artifact_or_tree: &Tree) -> PathBuf {
    artifact_or_tree
        .object
        .path_in(&Store::open(store).expect("store"))
        .expect("in store")
}

fn text_of(store: &str, artifact: &Artifact) -> String {
    let path = artifact
        .object
        .path_in(&Store::open(store).expect("store"))
        .expect("in store");
    std::fs::read_to_string(path).expect("read report")
}

/// Runs `program` under qemu-user with `library_dir` searched first.
fn run_under_qemu(qemu: &str, program: &Path, library_dir: &Path) -> String {
    let out = Command::new(qemu)
        .arg(program)
        .env("LD_LIBRARY_PATH", library_dir)
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "{} failed under qemu",
        program.display()
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn an_unused_library_is_reported() {
    let Some(cc) = tool(&["cc", "gcc"], "a C compiler") else {
        return;
    };
    let root = tempfile::tempdir().expect("tempdir");
    let (store, tree) = fixture(&cc, root.path());
    let report = unused_libs(&store, tree, &Ctx::new()).expect("report");
    assert_eq!(report.kind.as_str(), "report");
    let text = text_of(&store, &report);
    println!("{text}");
    assert!(
        text.contains("unused usr/lib/libunused.so.1"),
        "unused is reported:\n{text}"
    );
    assert!(
        text.contains("used usr/lib/libused.so.1 by usr/bin/app"),
        "used is reported:\n{text}"
    );
    assert!(
        !text.contains("unused usr/lib/libused.so.1"),
        "a needed library is not unused:\n{text}"
    );
    assert!(
        text.contains("missing libc.so.6 needed by usr/bin/app"),
        "a name the tree lacks is missing:\n{text}"
    );
}

#[test]
fn a_stripped_binary_still_runs_under_qemu_user() {
    let Some(cc) = tool(&["cc", "gcc"], "a C compiler") else {
        return;
    };
    let Some(strip) = tool(&["strip", "llvm-strip"], "strip") else {
        return;
    };
    let Some(qemu) = qemu() else {
        return;
    };
    let root = tempfile::tempdir().expect("tempdir");
    let (store, tree) = fixture(&cc, root.path());
    let before = std::fs::metadata(placed(&store, &tree).join("usr/bin/app"))
        .expect("app")
        .len();
    let stripped = strip_elf(&store, &strip, tree, &Ctx::new()).expect("strips");
    let dir = placed(&store, &stripped);
    let after = std::fs::metadata(dir.join("usr/bin/app"))
        .expect("app")
        .len();
    println!("usr/bin/app: {before} bytes, stripped {after} bytes");
    assert!(after < before, "stripping takes the debug data");
    let output = run_under_qemu(&qemu, &dir.join("usr/bin/app"), &dir.join("usr/lib"));
    assert_eq!(output, "app 7\n");
}

#[test]
fn pruning_removes_the_unused_library_and_its_link_and_the_program_still_runs() {
    let Some(cc) = tool(&["cc", "gcc"], "a C compiler") else {
        return;
    };
    let Some(qemu) = qemu() else {
        return;
    };
    let root = tempfile::tempdir().expect("tempdir");
    let (store, mut tree) = fixture(&cc, root.path());
    tree.entries = vec![
        FileEntry {
            path: "usr/lib/libunused.so.1".into(),
            mode: 0o644,
            uid: 0,
            gid: 0,
        },
        FileEntry {
            path: "usr/bin/app".into(),
            mode: 0o755,
            uid: 0,
            gid: 0,
        },
    ];
    let pruned = prune_libs(&store, tree, &Ctx::new()).expect("prunes");
    let dir = placed(&store, &pruned);
    assert!(!dir.join("usr/lib/libunused.so.1").exists(), "removed");
    assert!(
        dir.join("usr/lib/libunused.so").symlink_metadata().is_err(),
        "its link is removed"
    );
    assert!(dir.join("usr/lib/libused.so.1").exists(), "kept");
    assert!(dir.join("usr/lib/libused.so").symlink_metadata().is_ok());
    let paths: Vec<&str> = pruned.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(
        paths,
        ["usr/bin/app"],
        "the entry for the removed library is dropped"
    );
    assert_eq!(
        run_under_qemu(&qemu, &dir.join("usr/bin/app"), &dir.join("usr/lib")),
        "app 7\n"
    );
    let report = unused_libs(&store, pruned, &Ctx::new()).expect("report");
    let text = text_of(&store, &report);
    assert!(!text.contains("unused "), "nothing is left unused:\n{text}");
}
