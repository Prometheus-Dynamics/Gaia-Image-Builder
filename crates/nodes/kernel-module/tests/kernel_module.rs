//! A real kernel module build: hello-world built against a prepared kernel
//! build tree, placed with depmod, and its vermagic checked against the
//! release. A build against a different release string is refused.
//!
//! Gated: it runs only with `GAIA_KMOD_TESTS=1`, and then needs
//! `GAIA_KMOD_BUILD_DIR` (a prepared kernel build tree) and `GAIA_KMOD_MODULES`
//! (a `lib/modules` directory holding that kernel's `<release>/`). Without the
//! gate it prints SKIPPED. Optional: `GAIA_KMOD_CC` (default `/usr/bin/gcc`),
//! `GAIA_KMOD_CROSS` (a CROSS_COMPILE prefix, with `GAIA_KMOD_CC` the matching
//! compiler), `GAIA_KMOD_ARCH` (default `x86_64`, the host), `GAIA_KMOD_CPUS`
//! (a CPU list, `taskset` at `/usr/bin/taskset`), `GAIA_KMOD_JOBS` (default 4).

use gaia_kernel_module::{KernelModuleHost, kernel_module};
use gaia_step::{Ctx, Store, logs_dir, step_records};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const HELLO_C: &str = "\
#include <linux/module.h>
#include <linux/kernel.h>
MODULE_LICENSE(\"GPL\");
MODULE_DESCRIPTION(\"Gaia D3 hello-world proof module\");
static int __init hello_init(void) { pr_info(\"hello\\n\"); return 0; }
static void __exit hello_exit(void) { pr_info(\"bye\\n\"); }
module_init(hello_init);
module_exit(hello_exit);
";

const KBUILD: &str = "obj-m := hello.o\n";

fn gated() -> bool {
    if std::env::var_os("GAIA_KMOD_TESTS").is_none() {
        println!("SKIPPED: set GAIA_KMOD_TESTS=1 with GAIA_KMOD_BUILD_DIR and GAIA_KMOD_MODULES");
        return false;
    }
    true
}

fn env_path(name: &str) -> String {
    let value = std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"));
    assert!(Path::new(&value).is_absolute(), "{name} must be absolute");
    value
}

fn host() -> KernelModuleHost {
    let env = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
    KernelModuleHost {
        make: "/usr/bin/make".into(),
        cc: env("GAIA_KMOD_CC", "/usr/bin/gcc"),
        modinfo: "/usr/bin/modinfo".into(),
        depmod: "/usr/bin/depmod".into(),
        arch: env("GAIA_KMOD_ARCH", "x86_64"),
        cross_compile: env("GAIA_KMOD_CROSS", ""),
        taskset: "/usr/bin/taskset".into(),
        cpus: env("GAIA_KMOD_CPUS", ""),
        jobs: env("GAIA_KMOD_JOBS", "4").parse().unwrap_or(4),
    }
}

/// Puts `dir` in the store and returns it as a tree.
fn in_store(store: &Store, dir: &Path) -> Tree {
    Tree {
        object: store.put_tree(dir).expect("into the store").into(),
        entries: Vec::new(),
    }
}

/// The module source, as a directory in the test's own scratch space.
fn module_source(scratch: &Path) -> PathBuf {
    let dir = scratch.join("hello");
    fs::create_dir_all(&dir).expect("module dir");
    fs::write(dir.join("hello.c"), HELLO_C).expect("hello.c");
    fs::write(dir.join("Kbuild"), KBUILD).expect("Kbuild");
    dir
}

/// Runs the node. Returns the outputs path and whether the body ran.
fn run(
    store_dir: &Path,
    module: Tree,
    build: Tree,
    modules: Tree,
    host: KernelModuleHost,
) -> Result<(PathBuf, bool), String> {
    let kernel_modules = Artifact {
        kind: ArtifactKind::parse("modules"),
        object: modules.object,
    };
    let out = kernel_module(
        &store_dir.display().to_string(),
        module,
        build,
        kernel_modules,
        vec!["hello".into()],
        host,
        &Ctx::new(),
    )
    .map_err(|error| error.to_string())?;
    let store = Store::open(store_dir).expect("store");
    let made = step_records(&logs_dir(&store))
        .into_iter()
        .rev()
        .find(|record| record.step == "gaia.kernel-module")
        .expect("a step record")
        .made;
    let path = store.path(&out.object.object_ref().expect("an object"));
    Ok((path, made))
}

fn vermagic(ko: &Path) -> String {
    let out = Command::new("/usr/bin/modinfo")
        .arg("-F")
        .arg("vermagic")
        .arg(ko)
        .output()
        .expect("modinfo");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn hello_world_builds_against_the_kernel_and_its_vermagic_matches() {
    if !gated() {
        return;
    }
    let build_dir = PathBuf::from(env_path("GAIA_KMOD_BUILD_DIR"));
    let modules_root = PathBuf::from(env_path("GAIA_KMOD_MODULES"));
    let release = fs::read_to_string(build_dir.join("include/config/kernel.release"))
        .expect("kernel.release")
        .trim()
        .to_string();
    let dir = tempfile::tempdir().expect("tempdir");
    let store_dir = dir.path().join("store");
    let store = Store::open(&store_dir).expect("store");
    let source = module_source(dir.path());
    let module = in_store(&store, &source);
    let build = in_store(&store, &build_dir);
    let modules = in_store(&store, &staged_modules(dir.path(), &modules_root, &release));
    drop(store);

    let (out, made) = run(
        &store_dir,
        module.clone(),
        build.clone(),
        modules.clone(),
        host(),
    )
    .expect("the module build");
    println!("first build made={made}");
    assert!(made, "a first build makes its output");
    let ko = out.join(&release).join("updates/hello.ko");
    assert!(ko.is_file(), "hello.ko is placed under updates/");
    let got = vermagic(&ko);
    println!("modinfo -F vermagic {}: {got}", ko.display());
    assert_eq!(
        got.split_whitespace().next(),
        Some(release.as_str()),
        "vermagic names the kernel's release"
    );
    let dep = fs::read_to_string(out.join(&release).join("modules.dep")).expect("modules.dep");
    assert!(
        dep.contains("updates/hello.ko"),
        "depmod indexes the module: {dep}"
    );

    let (_, again) = run(
        &store_dir,
        module.clone(),
        build.clone(),
        modules.clone(),
        host(),
    )
    .expect("the same inputs");
    println!("same inputs again: made={again}");
    assert!(!again, "the same inputs are found in the store");
}

#[test]
fn a_build_for_a_different_release_string_is_refused() {
    if !gated() {
        return;
    }
    let build_dir = PathBuf::from(env_path("GAIA_KMOD_BUILD_DIR"));
    let modules_root = PathBuf::from(env_path("GAIA_KMOD_MODULES"));
    let release = fs::read_to_string(build_dir.join("include/config/kernel.release"))
        .expect("kernel.release")
        .trim()
        .to_string();
    let dir = tempfile::tempdir().expect("tempdir");
    // A copy of the build tree whose UTS_RELEASE is another string: the
    // module it builds carries that string as its vermagic.
    let mutant = dir.path().join("build");
    copy_dir(&build_dir, &mutant);
    let header = mutant.join("include/generated/utsrelease.h");
    let text = fs::read_to_string(&header).expect("utsrelease.h");
    fs::write(&header, text.replace(&release, "0.0.0-gaia-mutant")).expect("mutate");
    let store_dir = dir.path().join("store");
    let store = Store::open(&store_dir).expect("store");
    let source = module_source(dir.path());
    let module = in_store(&store, &source);
    let build = in_store(&store, &mutant);
    let modules = in_store(&store, &staged_modules(dir.path(), &modules_root, &release));
    drop(store);

    let error =
        run(&store_dir, module, build, modules, host()).expect_err("the mutant build is refused");
    println!("mutant build refused: {error}");
    assert!(
        error.contains("0.0.0-gaia-mutant") && error.contains(&release),
        "{error}"
    );
}

/// A `lib/modules` root holding only `release`, copied from the host's:
/// the node takes one release directory, and the host has several.
fn staged_modules(scratch: &Path, modules_root: &Path, release: &str) -> PathBuf {
    let root = scratch.join("modules");
    copy_dir(&modules_root.join(release), &root.join(release));
    root
}

/// A recursive copy for the mutant build tree: symlinks as links.
fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("mkdir");
    for entry in fs::read_dir(from).expect("read_dir").flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type().expect("type");
        if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&src).expect("link"), &dst).expect("link");
        } else if kind.is_dir() {
            copy_dir(&src, &dst);
        } else {
            fs::copy(&src, &dst).expect("copy");
        }
    }
}
