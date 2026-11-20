//! An out-of-tree Rust module built through the node against a kernel built
//! with LLVM and Rust (a board with `toolchain = "llvm"` and
//! `CONFIG_RUST=y`): it builds, its vermagic is the kernel's release, and it
//! is placed with depmod's index. The same kernel with the board's
//! toolchain declared gcc is refused, naming both, and so is a kernel build
//! for another release than the image's modules.
//!
//! Gated: it runs only with `GAIA_RUST_KERNEL_RUN`, a run record (the JSON a
//! build names) whose Buildroot steps include a kernel built so, and
//! `GAIA_STORE`, that build's store. Without them it prints SKIPPED.

use gaia_kernel_module::kernel_module;
use gaia_step::{Ctx, Store, logs_dir};
use gaia_types::{Artifact, ArtifactKind, BuildrootHost, Tree};
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

fn host() -> BuildrootHost {
    BuildrootHost {
        make: "/usr/bin/make".into(),
        taskset: "/usr/bin/taskset".into(),
        cpus: std::env::var("GAIA_BUILDROOT_CPUS").unwrap_or_default(),
        jobs: 8,
        download_dir: String::new(),
        download_pool: String::new(),
        ccache_dir: String::new(),
        mirror: String::new(),
        config_only: false,
        dev_reuse: String::new(),
        bwrap: ["/usr/bin/bwrap"]
            .into_iter()
            .find(|p| std::path::Path::new(p).exists())
            .unwrap_or_default()
            .into(),
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
    let build = tree(&store, &kernel.kernel_build);
    let run_node = |declared: &str, build: &Tree| {
        kernel_module(
            &store_dir.display().to_string(),
            tree(&store, &src),
            build.clone(),
            tree(&store, &kernel.toolchain),
            declared.into(),
            modules.clone(),
            "gaia_hello".into(),
            &host(),
            &Ctx::new(),
        )
        .map_err(|error| error.to_string())
    };

    let why = run_node("gcc", &build).expect_err("a clang kernel for a gcc board");
    println!("gcc: {why}");
    assert!(
        why.contains(
            "the board's kernel toolchain is gcc, but the kernel build was made with clang"
        ),
        "{why}"
    );

    let made = run_node("llvm", &build).expect("the Rust module builds");
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

    // The make that built it named every Rust tool, the toolchain's, and
    // none from a user's PATH.
    let make_line = newest_make_line(&logs_dir(&store));
    let toolchain = kernel.toolchain.display().to_string();
    for word in [
        format!("BINDGEN={toolchain}/bin/bindgen"),
        format!("RUSTC={toolchain}/bin/rustc"),
        format!("HOSTRUSTC={toolchain}/bin/rustc"),
        format!("LLVM={toolchain}/opt/llvm-prebuilt/bin/"),
    ] {
        assert!(make_line.contains(&word), "{word} in {make_line}");
    }
    assert!(!make_line.contains(".cargo"), "{make_line}");
    for tool in ["bin/bindgen", "bin/rustc"] {
        let out = std::process::Command::new(kernel.toolchain.join(tool))
            .arg("--version")
            .env_clear()
            .output()
            .expect("version");
        println!(
            "ran {tool}: {}",
            String::from_utf8_lossy(&out.stdout).trim()
        );
    }
    println!("make: {make_line}");

    // The planted fault: a kernel build for another release than the
    // image's modules. Its module would not load, and the node refuses it.
    let release = release.to_string_lossy().into_owned();
    let mutant_dir = scratch.path().join("kernel-build-mutant");
    copy_dir(&kernel.kernel_build, &mutant_dir);
    let header = mutant_dir.join("include/generated/utsrelease.h");
    let text = fs::read_to_string(&header).expect("utsrelease.h");
    assert!(text.contains(&release), "{text}");
    fs::write(&header, text.replace(&release, "0.0.0-gaia-mutant")).expect("plant");
    let why = run_node("llvm", &tree(&store, &mutant_dir)).expect_err("another release");
    println!("another release: {why}");
    assert!(
        why.contains("0.0.0-gaia-mutant") && why.contains(&release),
        "{why}"
    );
}

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
            gaia_store::copy_file(&src, &dst).expect("copy");
        }
    }
}

/// The command line of the newest module make in the store's run logs.
fn newest_make_line(logs: &Path) -> String {
    let mut lines: Vec<(String, String)> = fs::read_dir(logs.join("gaia.kernel-module"))
        .expect("the node's run logs")
        .flatten()
        .filter_map(|entry| {
            let text = fs::read_to_string(entry.path()).ok()?;
            let tool = text.lines().find_map(|line| line.strip_prefix("# tool "))?;
            tool.ends_with(" modules").then(|| {
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    tool.to_string(),
                )
            })
        })
        .collect();
    lines.sort();
    lines.pop().expect("a module make's run log").1
}
