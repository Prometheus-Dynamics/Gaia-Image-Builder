//! `gaia.kernel-module`: Kbuild modules built against a kernel build tree,
//! placed for the image's kernel with `depmod`'s index.
//!
//! Inputs:
//! - **module**: the module's source tree, with a `Kbuild` or `Makefile`.
//! - **kernel-build**: the kernel build tree Buildroot's `kernel-build` output
//!   gives (`.config`, `Module.symvers`, `include/generated/utsrelease.h`,
//!   `scripts/`, the headers). It is read, never written: make builds the module
//!   in the step's own copy of the source.
//! - **toolchain**: the cross toolchain that kernel was built with, Buildroot's
//!   `toolchain` output. The compiler is `bin/<cross-compile>gcc` inside it, and
//!   its directory leads PATH. No compiler path comes from the host.
//! - **kernel-toolchain**: the board's declared kernel toolchain, `gcc` or
//!   `llvm` (`[kernel] toolchain`). The kernel build's `.config` must agree
//!   (`CONFIG_CC_IS_CLANG=y` is llvm), or the step refuses. With `llvm` the
//!   module is built as the kernel was: `LLVM=` the toolchain's
//!   `opt/llvm-prebuilt/bin/` (clang, lld and the LLVM tools; no
//!   `CROSS_COMPILE`), and, when the kernel has Rust (`CONFIG_RUST=y`),
//!   `RUSTC=` its `bin/rustc`.
//! - **kernel-modules**: the image's kernel modules, Buildroot's `modules`
//!   artifact (`<release>/...`).
//! - **names**: the module names Kbuild builds (`obj-m`), each giving
//!   `<name>.ko`.
//! - **host**: make, modinfo, depmod, the architecture, the cross prefix, the
//!   job count and the CPU pin.
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
//! The key holds the digests of the module source, the kernel build, the
//! toolchain and the modules, the names, the architecture, the cross prefix,
//! the kernel toolchain and the version of each tool (make, the compiler,
//! rustc for a Rust kernel, modinfo, depmod), read once per process. The job count and CPU pin change how fast it runs, not what it
//! makes.

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
/// 2: the kernel toolchain, and modules built with LLVM.
const KEY_VERSION: u32 = 2;
/// Where the toolchain tree holds the pinned LLVM (Gaia's Buildroot patch
/// 0006, `host-llvm-prebuilt`).
const LLVM_BIN: &str = "opt/llvm-prebuilt/bin";
/// The directories every tool's PATH ends with, after the toolchain's `bin`.
const BASE_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
/// Fixed build stamps, so the same source gives the same module bytes.
const BUILD_TIMESTAMP: &str = "Thu Jan  1 00:00:00 UTC 1970";
const BUILD_USER: &str = "gaia";
const BUILD_HOST: &str = "gaia";
/// The step's output directory, in the scratch space.
const OUTPUT: &str = "modules";

/// The host side of a module build: the host's make, modinfo and depmod, the
/// kernel's architecture and cross prefix, and the job count. The compiler is
/// not here: it comes from the toolchain input.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:kernel-module-host")]
pub struct KernelModuleHost {
    /// Absolute path of make.
    pub make: String,
    /// Absolute path of modinfo, which reads a module's vermagic.
    pub modinfo: String,
    /// Absolute path of depmod, which writes the module index.
    pub depmod: String,
    /// The kernel's `ARCH`, such as `arm64`.
    pub arch: String,
    /// The `CROSS_COMPILE` prefix of the toolchain's compiler, such as
    /// `aarch64-linux-`. The compiler is `bin/<prefix>gcc`.
    pub cross_compile: String,
    /// Absolute path of taskset, used when `cpus` is set.
    pub taskset: String,
    /// A CPU list such as `6-31`; empty for no pinning.
    pub cpus: String,
    /// Make jobs, at least 1.
    pub jobs: u32,
}

/// Builds the modules `names` from `module`, against the kernel build tree
/// `kernel-build` and the `toolchain` it was built with, and places them in a
/// copy of the image's `kernel-modules` with depmod's index. See the module docs.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
#[node(
    id = "gaia.kernel-module",
    inputs(
        store,
        module,
        kernel_build,
        toolchain,
        kernel_toolchain,
        kernel_modules,
        names,
        host
    ),
    outputs(modules)
)]
pub fn kernel_module(
    store: &str,
    module: Tree,
    kernel_build: Tree,
    toolchain: Tree,
    kernel_toolchain: String,
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
        in_store(&store, &kernel_build, "the kernel build tree").map_err(fail)?;
    let (tool_dir, tool_digest) = in_store(&store, &toolchain, "the toolchain").map_err(fail)?;
    let config = fs::read_to_string(build_dir.join(".config")).map_err(|error| {
        fail(format!(
            "{} is not a kernel build tree (no .config): {error}",
            build_dir.display()
        ))
    })?;
    let compile = module::compile_of(&config, &kernel_toolchain).map_err(fail)?;
    let bin = tool_dir.join("bin");
    let llvm = tool_dir.join(LLVM_BIN);
    let (cc, rustc) = match compile {
        module::Compile::Gcc => (bin.join(format!("{}gcc", host.cross_compile)), None),
        module::Compile::Llvm { rust } => (llvm.join("clang"), rust.then(|| bin.join("rustc"))),
    };
    if !cc.is_file() {
        return Err(fail(format!(
            "the toolchain has no compiler {} (kernel toolchain {kernel_toolchain}, cross-compile {:?})",
            cc.display(),
            host.cross_compile
        )));
    }
    if let Some(rustc) = rustc.as_ref().filter(|rustc| !rustc.is_file()) {
        return Err(fail(format!(
            "the kernel has Rust, and the toolchain has no rustc {}",
            rustc.display()
        )));
    }
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
    let compiler = tool_version(&path_string(&cc), &["--version"]).map_err(fail)?;
    let rustc_version = match &rustc {
        Some(rustc) => tool_version(&path_string(rustc), &["--version"]).map_err(fail)?,
        None => Vec::new(),
    };
    let modinfo = tool_version(&host.modinfo, &["--version"]).map_err(fail)?;
    let depmod = tool_version(&host.depmod, &["--version"]).map_err(fail)?;
    let key = KeyBuilder::new(NAME, KEY_VERSION)
        .config("release", &release)
        .config("arch", &host.arch)
        .config("cross", &host.cross_compile)
        .config("names", names.join("\n"))
        .config("tool make", make)
        .config("kernel toolchain", &kernel_toolchain)
        .config("tool cc", compiler)
        .config("tool rustc", rustc_version)
        .config("tool modinfo", modinfo)
        .config("tool depmod", depmod)
        .input("module", src_digest)
        .input("kernel-build", build_digest)
        .input("toolchain", tool_digest)
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
                &Compiler {
                    cc: &cc,
                    bin: &bin,
                    llvm: matches!(compile, module::Compile::Llvm { .. }).then_some(llvm.as_path()),
                    rustc: rustc.as_deref(),
                },
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

/// The compiler the kernel build uses, and the toolchain's `bin` for PATH.
/// For a kernel built with LLVM, the LLVM `bin` (`LLVM=`), and rustc when it
/// has Rust.
struct Compiler<'a> {
    cc: &'a Path,
    bin: &'a Path,
    llvm: Option<&'a Path>,
    rustc: Option<&'a Path>,
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

fn path_string(path: &Path) -> String {
    path.display().to_string()
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
#[allow(clippy::too_many_arguments)]
fn build_and_place(
    work: &mut Work<'_>,
    src: &Path,
    build: &Path,
    modules: &Path,
    compiler: &Compiler<'_>,
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

    work.run(&make_tool(host, build, &source, compiler))?;
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
                .env("PATH", path_env(compiler))
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
        gaia_store::copy_file(ko, &placed).map_err(io_fail("placing a module"))?;
    }

    work.run(
        &Tool::new(&host.depmod)
            .arg("-b")
            .arg(&stage)
            .arg(release)
            .env("PATH", path_env(compiler))
            .env("LC_ALL", "C")
            .env("HOME", dir.display().to_string()),
    )?;
    fs::rename(stage.join("lib/modules"), dir.join(OUTPUT))
        .map_err(io_fail("the output modules"))?;
    Ok(())
}

/// make, run for the module in `source` against `build`, with the toolchain's
/// compiler. With a CPU list, under `taskset`.
fn make_tool(
    host: &KernelModuleHost,
    build: &Path,
    source: &Path,
    compiler: &Compiler<'_>,
) -> Tool {
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
        .arg(format!("-j{}", host.jobs));
    for arg in compile_args(host, compiler) {
        tool = tool.arg(arg);
    }
    tool.arg("modules")
        .env("PATH", path_env(compiler))
        .env("LC_ALL", "C")
        .env("HOME", source.display().to_string())
        .env("KBUILD_BUILD_TIMESTAMP", BUILD_TIMESTAMP)
        .env("KBUILD_BUILD_USER", BUILD_USER)
        .env("KBUILD_BUILD_HOST", BUILD_HOST)
}

/// make's compiler words: as the kernel was built. gcc: `CC=` the
/// toolchain's and its `CROSS_COMPILE`. LLVM: `LLVM=` its `bin/` (clang takes
/// the target from `ARCH`), and `RUSTC=` for a kernel with Rust.
fn compile_args(host: &KernelModuleHost, compiler: &Compiler<'_>) -> Vec<String> {
    let Some(llvm) = compiler.llvm else {
        let mut args = vec![format!("CC={}", path_string(compiler.cc))];
        if !host.cross_compile.is_empty() {
            args.push(format!("CROSS_COMPILE={}", host.cross_compile));
        }
        return args;
    };
    let mut args = vec![format!("LLVM={}/", path_string(llvm))];
    if let Some(rustc) = compiler.rustc {
        args.push(format!("RUSTC={}", path_string(rustc)));
    }
    args
}

/// The PATH every tool runs with: the LLVM `bin` for a kernel built with
/// LLVM, the toolchain's `bin`, then the base.
fn path_env(compiler: &Compiler<'_>) -> String {
    match compiler.llvm {
        Some(llvm) => format!(
            "{}:{}:{BASE_PATH}",
            path_string(llvm),
            path_string(compiler.bin)
        ),
        None => format!("{}:{BASE_PATH}", path_string(compiler.bin)),
    }
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&KERNEL_MODULE_NODE];

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> KernelModuleHost {
        KernelModuleHost {
            make: "/usr/bin/make".into(),
            modinfo: "/usr/bin/modinfo".into(),
            depmod: "/usr/bin/depmod".into(),
            arch: "arm64".into(),
            cross_compile: "aarch64-linux-".into(),
            taskset: "/usr/bin/taskset".into(),
            cpus: String::new(),
            jobs: 1,
        }
    }

    /// gcc: the toolchain's gcc and the cross prefix. LLVM: the LLVM prefix,
    /// no gcc and no cross prefix, rustc only for a kernel with Rust, and
    /// the LLVM `bin` first on PATH.
    #[test]
    fn a_module_is_made_with_the_compilers_its_kernel_was() {
        let (cc, bin) = (Path::new("/t/bin/aarch64-linux-gcc"), Path::new("/t/bin"));
        let gcc = Compiler {
            cc,
            bin,
            llvm: None,
            rustc: None,
        };
        assert_eq!(
            compile_args(&host(), &gcc),
            [
                "CC=/t/bin/aarch64-linux-gcc",
                "CROSS_COMPILE=aarch64-linux-"
            ]
        );
        assert_eq!(path_env(&gcc), format!("/t/bin:{BASE_PATH}"));
        let llvm = Path::new("/t/opt/llvm-prebuilt/bin");
        let clang = Path::new("/t/opt/llvm-prebuilt/bin/clang");
        let c = Compiler {
            cc: clang,
            bin,
            llvm: Some(llvm),
            rustc: None,
        };
        assert_eq!(
            compile_args(&host(), &c),
            ["LLVM=/t/opt/llvm-prebuilt/bin/"]
        );
        assert_eq!(
            path_env(&c),
            format!("/t/opt/llvm-prebuilt/bin:/t/bin:{BASE_PATH}")
        );
        let rust = Compiler {
            rustc: Some(Path::new("/t/bin/rustc")),
            ..c
        };
        assert_eq!(
            compile_args(&host(), &rust),
            ["LLVM=/t/opt/llvm-prebuilt/bin/", "RUSTC=/t/bin/rustc"]
        );
    }
}
