//! `gaia.strip.runtime-libs` on real ELF files, built with the host's C
//! compiler under the toolchain runtimes' names: a root where something links
//! libstdc++ keeps it, one where nothing does loses it, and nothing that is
//! not on a list is touched. Without a C compiler the tests say SKIPPED,
//! unless `GAIA_REQUIRE_TOOLS=1`, when they fail.

use gaia_step::{Ctx, Store};
use gaia_strip::runtime_libs;
use gaia_types::{FileEntry, Tree};
use std::path::{Path, PathBuf};
use std::process::Command;

fn cc() -> Option<String> {
    let found = [
        "/usr/bin/cc",
        "/usr/bin/gcc",
        "/usr/local/bin/cc",
        "/bin/cc",
    ]
    .into_iter()
    .find(|path| Path::new(path).exists());
    if found.is_none() {
        assert!(
            std::env::var_os("GAIA_REQUIRE_TOOLS").is_none(),
            "GAIA_REQUIRE_TOOLS is set, and a C compiler is missing"
        );
        println!("SKIPPED: a C compiler is not installed");
    }
    found.map(str::to_string)
}

fn run(program: &str, args: &[&str]) {
    let status = Command::new(program)
        .args(args)
        .status()
        .unwrap_or_else(|e| panic!("{program}: {e}"));
    assert!(status.success(), "{program} {args:?} failed");
}

/// A shared library `file` (its SONAME `soname`) in the tree's usr/lib, with
/// one function named after `symbol`, linked against `needs` (file names
/// there).
fn library(cc: &str, root: &Path, file: &str, soname: &str, symbol: &str, needs: &[&str]) {
    let lib = root.join("tree/usr/lib");
    std::fs::create_dir_all(&lib).expect("mkdir");
    let src = root.join(format!("{symbol}.c"));
    std::fs::write(&src, format!("int {symbol}(void) {{ return 1; }}\n")).expect("write");
    let lib_str = lib.display().to_string();
    let mut args = vec![
        "-shared".to_string(),
        "-fPIC".into(),
        format!("-Wl,-soname,{soname}"),
        "-Wl,--no-as-needed".into(),
        "-o".into(),
        format!("{lib_str}/{file}"),
        src.display().to_string(),
        format!("-L{lib_str}"),
    ];
    args.extend(needs.iter().map(|need| format!("-l:{need}")));
    run(cc, &args.iter().map(String::as_str).collect::<Vec<_>>());
}

/// A program at usr/bin/`name` linked against `needs`.
fn program(cc: &str, root: &Path, name: &str, needs: &[&str]) {
    let bin = root.join("tree/usr/bin");
    std::fs::create_dir_all(&bin).expect("mkdir");
    let src = root.join(format!("{name}.c"));
    std::fs::write(&src, "int main(void) { return 0; }\n").expect("write");
    let lib_str = root.join("tree/usr/lib").display().to_string();
    let mut args = vec![
        "-Wl,--no-as-needed".to_string(),
        "-o".into(),
        bin.join(name).display().to_string(),
        src.display().to_string(),
        format!("-L{lib_str}"),
    ];
    args.extend(needs.iter().map(|need| format!("-l:{need}")));
    run(cc, &args.iter().map(String::as_str).collect::<Vec<_>>());
}

/// A root as a prebuilt toolchain leaves it: libstdc++ (with its version
/// link and gdb file), libgfortran and libgomp that no program links, a C++
/// binding (libgpiodcxx) that needs libstdc++ and that nothing needs, a
/// libgcc_s nothing links, a plain library nothing links, and a program
/// linked against `app_needs`.
fn root(cc: &str, dir: &Path, app_needs: &[&str]) -> (String, Tree) {
    library(cc, dir, "libstdc++.so.6.0.34", "libstdc++.so.6", "cxx", &[]);
    let lib = dir.join("tree/usr/lib");
    std::os::unix::fs::symlink("libstdc++.so.6.0.34", lib.join("libstdc++.so.6")).expect("link");
    std::fs::write(lib.join("libstdc++.so.6.0.34-gdb.py"), "# gdb\n").expect("write");
    library(
        cc,
        dir,
        "libgfortran.so.5.0.0",
        "libgfortran.so.5",
        "fortran",
        &[],
    );
    std::os::unix::fs::symlink("libgfortran.so.5.0.0", lib.join("libgfortran.so.5")).expect("link");
    library(cc, dir, "libgomp.so.1.0.0", "libgomp.so.1", "gomp", &[]);
    library(
        cc,
        dir,
        "libgpiodcxx.so.2.1.1",
        "libgpiodcxx.so.2",
        "gpiodcxx",
        &["libstdc++.so.6"],
    );
    library(cc, dir, "libgcc_s.so.1", "libgcc_s.so.1", "gcc_s", &[]);
    library(cc, dir, "libplain.so.1", "libplain.so.1", "plain", &[]);
    program(cc, dir, "app", app_needs);
    let store_path = dir.join("store");
    let store = Store::open(&store_path).expect("store");
    let entry = |path: &str| FileEntry {
        path: path.into(),
        mode: 0o644,
        uid: 0,
        gid: 0,
        xattrs: Default::default(),
    };
    let tree = Tree {
        object: store.put_tree(&dir.join("tree")).expect("put").into(),
        entries: vec![entry("usr/lib/libgomp.so.1.0.0"), entry("usr/bin/app")],
    };
    (store_path.display().to_string(), tree)
}

fn pruned(
    store: &str,
    tree: &Tree,
    extra: &[(&str, &str)],
    keep: &[(&str, &str)],
) -> Result<(PathBuf, Vec<String>, String), String> {
    let json = |list: &[(&str, &str)]| {
        let items: Vec<String> = list
            .iter()
            .map(|(lib, reason)| format!("{{\"lib\":\"{lib}\",\"reason\":\"{reason}\"}}"))
            .collect();
        format!("[{}]", items.join(","))
    };
    let (extra, keep) = (json(extra), json(keep));
    let (out, report) = runtime_libs(store, tree.clone(), Some(&extra), Some(&keep), &Ctx::new())
        .map_err(|e| e.to_string())?;
    let opened = Store::open(store).expect("store");
    let dir = out.object.path_in(&opened).expect("in store");
    let text =
        std::fs::read_to_string(report.object.path_in(&opened).expect("in store")).expect("report");
    let entries = out.entries.iter().map(|e| e.path.clone()).collect();
    Ok((dir, entries, text))
}

fn has(dir: &Path, name: &str) -> bool {
    dir.join("usr/lib").join(name).symlink_metadata().is_ok()
}

#[test]
fn a_runtime_nothing_links_is_removed_and_one_something_links_is_kept() {
    let Some(cc) = cc() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, tree) = root(&cc, dir.path(), &[]);
    let (out, entries, report) = pruned(&store, &tree, &[], &[]).expect("prunes");
    println!("{report}");
    // Fortran and OpenMP: nothing needs them. Their links go with them, and
    // the entry of a removed file.
    for gone in [
        "libgfortran.so.5.0.0",
        "libgfortran.so.5",
        "libgomp.so.1.0.0",
    ] {
        assert!(!has(&out, gone), "{gone} is still there");
    }
    assert_eq!(entries, ["usr/bin/app"]);
    // libstdc++ stays: an ELF in the root needs it, although no program does.
    for kept in [
        "libstdc++.so.6.0.34",
        "libstdc++.so.6",
        "libstdc++.so.6.0.34-gdb.py",
        "libgpiodcxx.so.2.1.1",
    ] {
        assert!(has(&out, kept), "{kept} is gone");
    }
    assert!(
        report.contains("kept    libstdc++: needed by usr/lib/libgpiodcxx.so.2.1.1\n"),
        "{report}"
    );
    // Not on a list: never touched, linked or not.
    assert!(has(&out, "libgcc_s.so.1") && has(&out, "libplain.so.1"));
    assert!(
        !report.contains("libgcc_s") && !report.contains("libplain"),
        "{report}"
    );
    let size = |name: &str| {
        std::fs::metadata(dir.path().join("tree/usr/lib").join(name))
            .expect("stat")
            .len()
    };
    let fortran = size("libgfortran.so.5.0.0");
    assert!(
        report.contains(&format!(
            "removed libgfortran: a toolchain runtime library; no ELF in the root needs it: {fortran} B in usr/lib/libgfortran.so.5 usr/lib/libgfortran.so.5.0.0\n"
        )),
        "{report}"
    );
    assert!(
        report.ends_with(&format!(
            "saved   {} B in 3 files\n",
            fortran + size("libgomp.so.1.0.0")
        )),
        "{report}"
    );

    // A program that links libstdc++: kept, by its name.
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, tree) = root(&cc, dir.path(), &["libstdc++.so.6"]);
    let (out, _, report) = pruned(
        &store,
        &tree,
        &[("libgpiodcxx", "nothing here uses the C++ binding")],
        &[],
    )
    .expect("prunes");
    println!("{report}");
    assert!(has(&out, "libstdc++.so.6.0.34") && has(&out, "libstdc++.so.6"));
    assert!(!has(&out, "libgpiodcxx.so.2.1.1"));
    assert!(
        report.contains("kept    libstdc++: needed by usr/bin/app\n"),
        "{report}"
    );
}

#[test]
fn a_library_the_product_names_goes_first_and_what_only_it_needed_after() {
    let Some(cc) = cc() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, tree) = root(&cc, dir.path(), &[]);
    let (out, _, report) = pruned(
        &store,
        &tree,
        &[(
            "libgpiodcxx",
            "Buildroot builds the C++ binding whenever the toolchain has C++",
        )],
        &[("libgomp", "the camera plugin loads it by name")],
    )
    .expect("prunes");
    println!("{report}");
    for gone in [
        "libgpiodcxx.so.2.1.1",
        "libstdc++.so.6.0.34",
        "libstdc++.so.6",
        "libstdc++.so.6.0.34-gdb.py",
        "libgfortran.so.5.0.0",
    ] {
        assert!(!has(&out, gone), "{gone} is still there");
    }
    assert!(has(&out, "libgomp.so.1.0.0"), "kept by name");
    assert!(
        report.contains(
            "removed libgpiodcxx: named by the product: Buildroot builds the C++ binding whenever the toolchain has C++; no ELF in the root needs it: "
        ),
        "{report}"
    );
    assert!(
        report.contains(
            "removed libstdc++: a toolchain runtime library; no ELF in the root needs it (round 2: only removed libraries needed it): "
        ),
        "{report}"
    );
    assert!(
        report.contains(
            "kept    libgomp: the product keeps it: the camera plugin loads it by name\n"
        ),
        "{report}"
    );
}

#[test]
fn a_list_that_names_what_is_not_there_or_not_allowed_fails_by_name() {
    let Some(cc) = cc() else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, tree) = root(&cc, dir.path(), &[]);
    for (extra, keep, says) in [
        (
            vec![("libnothere", "x")],
            vec![],
            "extra libnothere: the root has no libnothere.so*; remove it from the list",
        ),
        (
            vec![],
            vec![("libquadmath", "x")],
            "keep libquadmath: the root has no libquadmath.so*; remove it from the list",
        ),
        (
            vec![],
            vec![("libplain", "x")],
            "keep libplain: not a toolchain runtime library this step removes",
        ),
        (
            vec![("libgcc_s", "x")],
            vec![],
            "extra libgcc_s: libgcc_s is loaded by name at run time",
        ),
        (
            vec![("libgomp", "x")],
            vec![],
            "extra libgomp: libgomp is a toolchain runtime library, removed already",
        ),
    ] {
        let error = pruned(&store, &tree, &extra, &keep).expect_err(says);
        assert!(error.contains(says), "{says}: {error}");
    }
}
