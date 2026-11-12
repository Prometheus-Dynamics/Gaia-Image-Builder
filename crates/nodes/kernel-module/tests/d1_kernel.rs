//! The D1 kernel proof: a Buildroot build with a kernel (6.18.7, the custom
//! config of Buildroot's qemu aarch64 virt board), then hello-world built
//! against that kernel's own build tree and toolchain, placed with depmod, its
//! vermagic checked against the release. A build against a different release
//! string is refused.
//!
//! Gated like D1's buildroot test: it runs only with `GAIA_BUILDROOT_TESTS=1`,
//! and needs `GAIA_BUILDROOT_SOURCE` (a Buildroot tree) and `GAIA_BUILDROOT_DL`
//! (a download cache). Optional: `GAIA_BUILDROOT_CPUS` (a CPU list, `taskset`
//! at `/usr/bin/taskset`) and `GAIA_BUILDROOT_JOBS` (default 13). Without the
//! gate it prints SKIPPED.

use gaia_kernel_module::{KernelModuleHost, kernel_module};
use gaia_os_buildroot::{BuildrootHost, ConfigFragment, os_buildroot};
use gaia_step::{Ctx, Store, logs_dir, step_records};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The minimal aarch64 Buildroot base the D1 test uses: busybox and a CPIO
/// root filesystem on Buildroot's bootlin toolchain.
const BASE: &str = "\
BR2_aarch64=y
BR2_TOOLCHAIN_EXTERNAL=y
BR2_TOOLCHAIN_EXTERNAL_BOOTLIN=y
BR2_TOOLCHAIN_EXTERNAL_BOOTLIN_AARCH64_GLIBC_BLEEDING_EDGE=y
BR2_PACKAGE_BUSYBOX=y
BR2_TARGET_ROOTFS_CPIO=y
BR2_TARGET_ROOTFS_CPIO_GZIP=y
";

/// The kernel options of `qemu_aarch64_virt_defconfig` (Buildroot 2026.02.3):
/// Linux 6.18.7 with its custom configuration file from the Buildroot tree.
const KERNEL: &str = "\
BR2_LINUX_KERNEL=y
BR2_LINUX_KERNEL_CUSTOM_VERSION=y
BR2_LINUX_KERNEL_CUSTOM_VERSION_VALUE=\"6.18.7\"
BR2_LINUX_KERNEL_USE_CUSTOM_CONFIG=y
BR2_LINUX_KERNEL_CUSTOM_CONFIG_FILE=\"board/qemu/aarch64-virt/linux.config\"
";

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
    if std::env::var_os("GAIA_BUILDROOT_TESTS").is_none() {
        println!(
            "SKIPPED: set GAIA_BUILDROOT_TESTS=1 with GAIA_BUILDROOT_SOURCE and GAIA_BUILDROOT_DL"
        );
        return false;
    }
    true
}

fn env_path(name: &str) -> String {
    let value = std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"));
    assert!(Path::new(&value).is_absolute(), "{name} must be absolute");
    value
}

fn jobs() -> u32 {
    std::env::var("GAIA_BUILDROOT_JOBS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(13)
}

fn cpus() -> String {
    std::env::var("GAIA_BUILDROOT_CPUS").unwrap_or_default()
}

fn buildroot_host() -> BuildrootHost {
    BuildrootHost {
        make: "/usr/bin/make".into(),
        taskset: "/usr/bin/taskset".into(),
        cpus: cpus(),
        jobs: jobs(),
        download_dir: env_path("GAIA_BUILDROOT_DL"),
    }
}

fn module_host() -> KernelModuleHost {
    KernelModuleHost {
        make: "/usr/bin/make".into(),
        modinfo: "/usr/bin/modinfo".into(),
        depmod: "/usr/bin/depmod".into(),
        arch: "arm64".into(),
        cross_compile: "aarch64-linux-".into(),
        taskset: "/usr/bin/taskset".into(),
        cpus: cpus(),
        jobs: jobs(),
    }
}

fn module_source(scratch: &Path) -> PathBuf {
    let dir = scratch.join("hello");
    fs::create_dir_all(&dir).expect("module dir");
    fs::write(dir.join("hello.c"), HELLO_C).expect("hello.c");
    fs::write(dir.join("Kbuild"), KBUILD).expect("Kbuild");
    dir
}

fn tree(store: &Store, dir: &Path) -> Tree {
    Tree {
        object: store.put_tree(dir).expect("into the store").into(),
        entries: Vec::new(),
    }
}

/// Runs the kernel-module node. Returns its output's path and whether its
/// body ran.
fn run_module(
    store_dir: &Path,
    module: Tree,
    kernel_build: Tree,
    toolchain: Tree,
    modules: Artifact,
) -> Result<(PathBuf, bool), String> {
    let out = kernel_module(
        &store_dir.display().to_string(),
        module,
        kernel_build,
        toolchain,
        modules,
        vec!["hello".into()],
        module_host(),
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

/// A recursive copy: symlinks as links.
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

#[test]
fn hello_world_builds_against_the_d1_kernel_and_its_vermagic_matches() {
    if !gated() {
        return;
    }
    let source = PathBuf::from(env_path("GAIA_BUILDROOT_SOURCE"));
    let dir = tempfile::tempdir().expect("tempdir");
    let store_dir = dir.path().join("store");
    let store = Store::open(&store_dir).expect("store");
    let source_tree = tree(&store, &source);
    drop(store);

    let fragments = vec![
        ConfigFragment {
            name: "minimal-aarch64".into(),
            text: BASE.into(),
        },
        ConfigFragment {
            name: "kernel".into(),
            text: KERNEL.into(),
        },
    ];
    let (
        _rootfs,
        _kernel,
        _device_trees,
        modules,
        _bootloader,
        _images,
        config,
        kernel_build,
        toolchain,
    ) = os_buildroot(
        &store_dir.display().to_string(),
        source_tree,
        fragments,
        Vec::new(),
        buildroot_host(),
        &Ctx::new(),
    )
    .expect("the Buildroot step with a kernel");
    let store = Store::open(&store_dir).expect("store");
    let kernel_build_path = store.path(&kernel_build.object.object_ref().expect("in the store"));
    let toolchain_path = store.path(&toolchain.object.object_ref().expect("in the store"));
    let config_text = fs::read_to_string(store.path(&config.object.object_ref().expect("config")))
        .expect("the config");
    assert!(
        config_text.contains("BR2_LINUX_KERNEL=y"),
        "the build has a kernel"
    );
    let release = fs::read_to_string(kernel_build_path.join("include/config/kernel.release"))
        .expect("kernel.release")
        .trim()
        .to_string();
    println!("D1 kernel release: {release}");
    println!(
        "toolchain compiler: {}",
        toolchain_path.join("bin/aarch64-linux-gcc").is_file()
    );
    let module = tree(&store, &module_source(dir.path()));
    drop(store);
    assert_eq!(
        kernel_module_release_dirs(&modules, &store_dir),
        vec![release.clone()],
        "the D1 modules are for the same release"
    );

    let modules_artifact = Artifact {
        kind: ArtifactKind::parse("modules"),
        object: modules.object.clone(),
    };
    let (out, made) = run_module(
        &store_dir,
        module.clone(),
        kernel_build.clone(),
        toolchain.clone(),
        modules_artifact.clone(),
    )
    .expect("the module build");
    println!("first module build made={made}");
    assert!(made, "a first build makes its output");
    let ko = out.join(&release).join("updates/hello.ko");
    assert!(ko.is_file(), "hello.ko is placed under updates/");
    let vermagic = Command::new("/usr/bin/modinfo")
        .arg("-F")
        .arg("vermagic")
        .arg(&ko)
        .output()
        .expect("modinfo");
    let got = String::from_utf8_lossy(&vermagic.stdout).trim().to_string();
    println!("modinfo -F vermagic {}: {got}", ko.display());
    assert_eq!(
        got.split_whitespace().next(),
        Some(release.as_str()),
        "vermagic names the D1 kernel's release"
    );
    let dep = fs::read_to_string(out.join(&release).join("modules.dep")).expect("modules.dep");
    assert!(dep.contains("updates/hello.ko"), "depmod indexes it: {dep}");

    let (_, again) = run_module(
        &store_dir,
        module.clone(),
        kernel_build.clone(),
        toolchain.clone(),
        modules_artifact.clone(),
    )
    .expect("the same inputs");
    println!("same inputs again: made={again}");
    assert!(!again, "the same inputs are found in the store");

    // The mutation: the build's UTS_RELEASE names another release. The node
    // refuses it, since the image's modules are for the D1 kernel's release.
    let mutant_dir = dir.path().join("kernel-build-mutant");
    copy_dir(&kernel_build_path, &mutant_dir);
    let header = mutant_dir.join("include/generated/utsrelease.h");
    let text = fs::read_to_string(&header).expect("utsrelease.h");
    fs::write(&header, text.replace(&release, "0.0.0-gaia-mutant")).expect("mutate");
    let store = Store::open(&store_dir).expect("store");
    let mutant = tree(&store, &mutant_dir);
    drop(store);
    let error = run_module(&store_dir, module, mutant, toolchain, modules_artifact)
        .expect_err("the mutant build is refused");
    println!("mutant build refused: {error}");
    assert!(
        error.contains("0.0.0-gaia-mutant") && error.contains(&release),
        "{error}"
    );
}

/// The release directories in a modules artifact, by name.
fn kernel_module_release_dirs(modules: &Artifact, store_dir: &Path) -> Vec<String> {
    let store = Store::open(store_dir).expect("store");
    let path = store.path(&modules.object.object_ref().expect("in the store"));
    let mut names: Vec<String> = fs::read_dir(path)
        .expect("modules")
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}
