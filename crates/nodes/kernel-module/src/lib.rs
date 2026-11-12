//! `gaia.kernel-module`: Kbuild modules built against a kernel build tree,
//! placed for the image's kernel with `depmod`'s index.
//!
//! Inputs:
//! - **module**: the module's source tree, with a `Kbuild` or `Makefile`.
//! - **build**: a prepared kernel build tree (`make modules_prepare` has run:
//!   `.config`, `Module.symvers`, `include/generated/utsrelease.h`, `scripts/`).
//!   It is read, never written: make builds the module in the step's own copy
//!   of the source.
//! - **kernel-modules**: the image's kernel modules, Buildroot's `modules`
//!   artifact (`<release>/...`).
//! - **names**: the module names Kbuild builds (`obj-m`), each giving
//!   `<name>.ko`.
//! - **host**: make, the compiler the kernel was built with, modinfo, depmod,
//!   the architecture, the cross prefix, the job count and the CPU pin.
//!
//! Output **modules**: the same shape as Buildroot's `modules` artifact,
//! `<release>/` with the input set, each new module under `updates/`, and
//! the `modules.*` index that `depmod` writes over all of it. Placed at
//! `lib/modules` in the image, `modprobe` finds it.
//!
//! Two checks say the module is for this kernel: the build's `UTS_RELEASE`
//! must be the release directory of the modules, and each built module's
//! vermagic must start with that release.
//!
//! The key holds the build's and the modules' digests, the module source's
//! digest, the names, the architecture, the cross prefix and the version of
//! each tool (make, the compiler, modinfo, depmod), read once per process.
//! The job count and CPU pin change how fast it runs, not what it makes.

mod copy;
mod module;

use daedalus::prelude::*;
use gaia_step::{Ctx, Digest, KeyBuilder, Step, StepError, Store, Tool, Work, once_per_process};
use gaia_types::{Artifact, ArtifactKind, Tree};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const NAME: &str = "gaia.kernel-module";
const KEY_VERSION: u32 = 1;
/// The environment every tool runs with. Nothing else is inherited.
const BASE_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
/// Fixed build stamps, so the same source gives the same module bytes.
const BUILD_TIMESTAMP: &str = "Thu Jan  1 00:00:00 UTC 1970";
const BUILD_USER: &str = "gaia";
const BUILD_HOST: &str = "gaia";
/// The step's output directory, in the scratch space.
const OUTPUT: &str = "modules";

/// The host side of a module build: the kernel's tools, the architecture and
/// the job count. Every tool is an absolute path.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:kernel-module-host")]
pub struct KernelModuleHost {
    /// Absolute path of make.
    pub make: String,
    /// Absolute path of the C compiler the kernel was built with (`CC`).
    pub cc: String,
    /// Absolute path of modinfo, which reads a module's vermagic.
    pub modinfo: String,
    /// Absolute path of depmod, which writes the module index.
    pub depmod: String,
    /// The kernel's `ARCH`, such as `arm64`.
    pub arch: String,
    /// The `CROSS_COMPILE` prefix, such as `aarch64-linux-gnu-`. Empty for a
    /// native build.
    pub cross_compile: String,
    /// Absolute path of taskset, used when `cpus` is set.
    pub taskset: String,
    /// A CPU list such as `6-31`; empty for no pinning.
    pub cpus: String,
    /// Make jobs, at least 1.
    pub jobs: u32,
}

/// Builds the modules `names` from `module`, against the kernel build tree
/// `build`, and places them in a copy of the image's `kernel-modules` with
/// depmod's index. See the module docs.
#[allow(clippy::type_complexity)]
#[node(
    id = "gaia.kernel-module",
    inputs(store, module, build, kernel_modules, names, host),
    outputs(modules)
)]
pub fn kernel_module(
    store: &str,
    module: Tree,
    build: Tree,
    kernel_modules: Artifact,
    names: Vec<String>,
    host: KernelModuleHost,
    cx: &Ctx,
) -> Result<Artifact, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |why: String| StepError::body(NAME, why);
    check(&names, &host).map_err(fail)?;
    let (src, src_digest) = in_store(&store, &module, "the module source").map_err(fail)?;
    let (build_dir, build_digest) =
        in_store(&store, &build, "the kernel build tree").map_err(fail)?;
    let modules_object = kernel_modules
        .object
        .object_ref()
        .ok_or_else(|| fail("the kernel modules are not in the store".into()))?;
    if kernel_modules.kind != ArtifactKind::parse("modules") {
        return Err(fail(format!(
            "kernel-modules must be a modules artifact, not {:?}",
            kernel_modules.kind
        )));
    }
    let modules_dir = store.path(&modules_object);

    let release = release_of_build(&build_dir).map_err(fail)?;
    let modules_release = single_release(&modules_dir).map_err(fail)?;
    if modules_release != release {
        return Err(fail(format!(
            "the kernel build is for release {release:?}, but the image's kernel modules are for {modules_release:?}"
        )));
    }

    let make = tool_version(&host.make, &["--version"]).map_err(fail)?;
    let cc = tool_version(&host.cc, &["--version"]).map_err(fail)?;
    let modinfo = tool_version(&host.modinfo, &["--version"]).map_err(fail)?;
    let depmod = tool_version(&host.depmod, &["--version"]).map_err(fail)?;
    let key = KeyBuilder::new(NAME, KEY_VERSION)
        .config("release", &release)
        .config("arch", &host.arch)
        .config("cross", &host.cross_compile)
        .config("names", names.join("\n"))
        .config("tool make", make)
        .config("tool cc", cc)
        .config("tool modinfo", modinfo)
        .config("tool depmod", depmod)
        .input("module", src_digest)
        .input("build", build_digest)
        .input("modules", modules_object.digest)
        .finish();

    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[(OUTPUT, Path::new(OUTPUT))],
        |work| {
            build_and_place(
                work,
                &src,
                &build_dir,
                &modules_dir,
                &release,
                &names,
                &host,
            )
        },
    )?;
    Ok(Artifact {
        kind: ArtifactKind::parse("modules"),
        object: made.outputs[OUTPUT].into(),
    })
}

fn check(names: &[String], host: &KernelModuleHost) -> Result<(), String> {
    if names.is_empty() {
        return Err("no module names: the node builds nothing without one".into());
    }
    for (i, name) in names.iter().enumerate() {
        module::valid_name(name)?;
        if names[..i].contains(name) {
            return Err(format!("module {name:?} is named twice"));
        }
    }
    if host.jobs == 0 {
        return Err("host jobs must be at least 1".into());
    }
    let mut tools = vec![
        ("make", &host.make),
        ("cc", &host.cc),
        ("modinfo", &host.modinfo),
        ("depmod", &host.depmod),
    ];
    if !host.cpus.is_empty() {
        tools.push(("taskset", &host.taskset));
    }
    for (what, path) in tools {
        if !Path::new(path).is_absolute() {
            return Err(format!("{what} {path:?} is not an absolute path"));
        }
    }
    Ok(())
}

/// A tree in the store: its path there and its digest.
fn in_store(store: &Store, tree: &Tree, what: &str) -> Result<(PathBuf, Digest), String> {
    let object = tree
        .object
        .object_ref()
        .ok_or_else(|| format!("{what} is not in the store"))?;
    Ok((store.path(&object), object.digest))
}

/// The release the kernel build is for, from its `utsrelease.h`.
fn release_of_build(build: &Path) -> Result<String, String> {
    let header = build.join("include/generated/utsrelease.h");
    let text = fs::read_to_string(&header).map_err(|error| {
        format!(
            "{} is not a prepared kernel build tree (no {}): {error}",
            build.display(),
            header.display()
        )
    })?;
    module::release_of_header(&text)
}

/// The one release directory in the modules tree.
fn single_release(modules: &Path) -> Result<String, String> {
    let mut names = Vec::new();
    for entry in
        fs::read_dir(modules).map_err(|error| format!("reading the kernel modules: {error}"))?
    {
        let entry = entry.map_err(|error| format!("reading the kernel modules: {error}"))?;
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    module::single_release(&names)
}

/// A tool's version output, read once per process: a failed run is not kept.
fn tool_version(program: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    once_per_process(Path::new(program), || {
        let output = Command::new(program).args(args).env_clear().output()?;
        Ok([output.stdout, output.stderr].concat())
    })
    .map_err(|error| format!("{program} {}: {error}", args.join(" ")))
}

/// The body of the step: copy the source into scratch, build each module
/// against the build tree, check its vermagic, then place it in a copy of the
/// image's kernel modules and index the set with depmod.
fn build_and_place(
    work: &mut Work<'_>,
    src: &Path,
    build: &Path,
    modules: &Path,
    release: &str,
    names: &[String],
    host: &KernelModuleHost,
) -> Result<(), StepError> {
    let fail = |why: String| StepError::body(NAME, why);
    let io_fail = |what: &'static str| {
        move |error: io::Error| StepError::body(NAME, format!("{what}: {error}"))
    };
    let dir = work.dir().to_path_buf();
    let source = dir.join("src");
    copy::copy_tree(src, &source).map_err(io_fail("copying the module source"))?;
    if !source.join("Kbuild").is_file() && !source.join("Makefile").is_file() {
        return Err(fail("the module source has no Kbuild or Makefile".into()));
    }

    work.run(&make_tool(host, build, &source))?;
    let mut built = Vec::with_capacity(names.len());
    for name in names {
        let ko = source.join(format!("{name}.ko"));
        if !ko.is_file() {
            return Err(fail(format!("make built no {name}.ko")));
        }
        let vermagic = work.run(
            &Tool::new(&host.modinfo)
                .arg("-F")
                .arg("vermagic")
                .arg(&ko)
                .env("PATH", path_env(host))
                .env("LC_ALL", "C")
                .env("HOME", dir.display().to_string()),
        )?;
        let line = vermagic.stdout_tail.first().map_or("", String::as_str);
        match module::vermagic_release(line) {
            Some(found) if found == release => {}
            found => {
                return Err(fail(format!(
                    "{name}.ko's vermagic is {found:?}, but the kernel build is for {release:?}"
                )));
            }
        }
        built.push((name.clone(), ko));
    }

    let stage = dir.join("stage");
    let stage_release = stage.join("lib/modules").join(release);
    fs::create_dir_all(stage.join("lib/modules")).map_err(io_fail("the staging directory"))?;
    copy::copy_tree(&modules.join(release), &stage_release)
        .map_err(io_fail("copying the kernel modules"))?;
    let updates = stage_release.join(module::UPDATES_DIR);
    fs::create_dir_all(&updates).map_err(io_fail("the updates directory"))?;
    for (name, ko) in &built {
        let placed = stage
            .join("lib/modules")
            .join(module::placed_path(release, name));
        fs::copy(ko, &placed).map_err(io_fail("placing a module"))?;
    }

    work.run(
        &Tool::new(&host.depmod)
            .arg("-b")
            .arg(&stage)
            .arg(release)
            .env("PATH", path_env(host))
            .env("LC_ALL", "C")
            .env("HOME", dir.display().to_string()),
    )?;
    fs::rename(stage.join("lib/modules"), dir.join(OUTPUT))
        .map_err(io_fail("the output modules"))?;
    Ok(())
}

/// make, run for the module in `source` against `build`. With a CPU list,
/// under `taskset`.
fn make_tool(host: &KernelModuleHost, build: &Path, source: &Path) -> Tool {
    let tool = if host.cpus.is_empty() {
        Tool::new(&host.make)
    } else {
        Tool::new(&host.taskset)
            .arg("-c")
            .arg(&host.cpus)
            .arg(&host.make)
    };
    let mut tool = tool
        .cwd(source)
        .arg("-C")
        .arg(build)
        .arg(format!("M={}", source.display()))
        .arg(format!("ARCH={}", host.arch))
        .arg(format!("CC={}", host.cc))
        .arg(format!("-j{}", host.jobs));
    if !host.cross_compile.is_empty() {
        tool = tool.arg(format!("CROSS_COMPILE={}", host.cross_compile));
    }
    tool.arg("modules")
        .env("PATH", path_env(host))
        .env("LC_ALL", "C")
        .env("HOME", source.display().to_string())
        .env("KBUILD_BUILD_TIMESTAMP", BUILD_TIMESTAMP)
        .env("KBUILD_BUILD_USER", BUILD_USER)
        .env("KBUILD_BUILD_HOST", BUILD_HOST)
}

/// The PATH every tool runs with: the compiler's directory, then the base.
fn path_env(host: &KernelModuleHost) -> String {
    match Path::new(&host.cc).parent() {
        Some(dir) if !dir.as_os_str().is_empty() => format!("{}:{BASE_PATH}", dir.display()),
        _ => BASE_PATH.to_string(),
    }
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&KERNEL_MODULE_NODE];
