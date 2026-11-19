//! An out-of-tree Rust module built through the node against a kernel built
//! with LLVM and Rust (a board with `toolchain = "llvm"` and
//! `CONFIG_RUST=y`): it builds, its vermagic is the kernel's release, and it
//! is placed with depmod's index. The same kernel with the board's
//! toolchain declared gcc is refused, naming both.
//!
//! Gated: it runs only with `GAIA_RUST_KERNEL_RUN`, a run record (the JSON a
//! build names) whose Buildroot steps include a kernel built so, and
//! `GAIA_STORE`, that build's store. Without them it prints SKIPPED.

use gaia_kernel_module::{KernelModuleHost, kernel_module};
use gaia_step::{Ctx, Store};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::fs;
use std::path::{Path, PathBuf};

const MODULE_RS: &str = r#"// SPDX-License-Identifier: GPL-2.0
//! Gaia's out-of-tree Rust module proof.
use kernel::prelude::*;

module! {
    type: GaiaHello,
    name: "gaia_hello",
    authors: ["Gaia"],
    description: "Gaia's out-of-tree Rust module proof",
    license: "GPL",
}

struct GaiaHello;

impl kernel::Module for GaiaHello {
    fn init(_module: &'static ThisModule) -> Result<Self> {
        pr_info!("gaia hello\n");
        Ok(GaiaHello)
    }
}
"#;

/// The kernel step's outputs a module build takes, as store paths.
struct KernelOutputs {
    kernel_build: PathBuf,
    toolchain: PathBuf,
    modules: PathBuf,
}

fn gated() -> Option<(PathBuf, PathBuf)> {
    match (
        std::env::var_os("GAIA_RUST_KERNEL_RUN"),
        std::env::var_os("GAIA_STORE"),
    ) {
        (Some(run), Some(store)) => Some((run.into(), store.into())),
        _ => {
            println!("SKIPPED: set GAIA_RUST_KERNEL_RUN (a run record) and GAIA_STORE");
            None
        }
    }
}

/// The outputs of the run's Buildroot step whose kernel build has Rust.
fn outputs(run: &Path, store: &Path) -> KernelOutputs {
    let text = fs::read_to_string(run).expect("the run record");
    let record: serde_json::Value = serde_json::from_str(&text).expect("JSON");
    let path = |object: &str| {
        let digest = object.rsplit(':').next().expect("a digest");
        let kind = object.split(':').next().expect("a kind");
        store
            .join("objects")
            .join(kind)
            .join(&digest[..2])
            .join(digest)
    };
    for step in record["steps"].as_array().expect("steps") {
        if step["step"] != "gaia.os.buildroot" {
            continue;
        }
        let out = &step["outputs"];
        let Some(build) = out["kernel-build"].as_str() else {
            continue;
        };
        let build = path(build);
        let config = fs::read_to_string(build.join(".config")).unwrap_or_default();
        if config.lines().any(|line| line == "CONFIG_RUST=y") {
            return KernelOutputs {
                kernel_build: build,
                toolchain: path(out["toolchain"].as_str().expect("toolchain")),
                modules: path(out["modules"].as_str().expect("modules")),
            };
        }
    }
    panic!(
        "no Buildroot step of {} built a kernel with CONFIG_RUST=y",
        run.display()
    );
}

fn tree(store: &Store, dir: &Path) -> Tree {
    Tree {
        object: store.put_tree(dir).expect("into the store").into(),
        entries: Vec::new(),
    }
}

fn host() -> KernelModuleHost {
    KernelModuleHost {
        make: "/usr/bin/make".into(),
        modinfo: "/usr/bin/modinfo".into(),
        depmod: "/usr/bin/depmod".into(),
        arch: "arm64".into(),
        cross_compile: String::new(),
        taskset: "/usr/bin/taskset".into(),
        cpus: std::env::var("GAIA_BUILDROOT_CPUS").unwrap_or_default(),
        jobs: 8,
    }
}

#[test]
fn a_rust_module_builds_against_an_llvm_kernel_and_gcc_is_refused() {
    let Some((run, store_dir)) = gated() else {
        return;
    };
    let kernel = outputs(&run, &store_dir);
    for needed in [
        "rust/libkernel.rmeta",
        "rust/libmacros.so",
        "Module.symvers",
    ] {
        assert!(
            kernel.kernel_build.join(needed).is_file(),
            "kernel-build has {needed}"
        );
    }
    let store = Store::open(&store_dir).expect("store");
    let scratch = tempfile::tempdir().expect("tempdir");
    let src = scratch.path().join("gaia_hello");
    fs::create_dir_all(&src).expect("mkdir");
    fs::write(src.join("gaia_hello.rs"), MODULE_RS).expect("source");
    fs::write(src.join("Kbuild"), "obj-m := gaia_hello.o\n").expect("Kbuild");
    let modules = Artifact {
        kind: ArtifactKind::parse("modules"),
        object: store.put_tree(&kernel.modules).expect("modules").into(),
    };
    let run_node = |declared: &str| {
        kernel_module(
            &store_dir.display().to_string(),
            tree(&store, &src),
            tree(&store, &kernel.kernel_build),
            tree(&store, &kernel.toolchain),
            declared.into(),
            modules.clone(),
            vec!["gaia_hello".into()],
            host(),
            &Ctx::new(),
        )
        .map_err(|error| error.to_string())
    };

    let why = run_node("gcc").expect_err("a clang kernel for a gcc board");
    println!("gcc: {why}");
    assert!(
        why.contains(
            "the board's kernel toolchain is gcc, but the kernel build was made with clang"
        ),
        "{why}"
    );

    let made = run_node("llvm").expect("the Rust module builds");
    let out = store.path(&made.object.object_ref().expect("an object"));
    let release = fs::read_dir(&out)
        .expect("release dir")
        .flatten()
        .next()
        .expect("one release")
        .file_name();
    let ko = out.join(&release).join("updates/gaia_hello.ko");
    assert!(ko.is_file(), "{} is placed", ko.display());
    let dep = fs::read_to_string(out.join(&release).join("modules.dep")).expect("modules.dep");
    assert!(dep.contains("updates/gaia_hello.ko"), "{dep}");
    println!(
        "built {} ({} bytes)",
        ko.display(),
        fs::metadata(&ko).expect("ko").len()
    );
}
