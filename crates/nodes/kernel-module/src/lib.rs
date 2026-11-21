//! `gaia.kernel-module`: Kbuild modules built against a kernel build tree,
//! placed for the image's kernel with `depmod`'s index.
//!
//! Inputs:
//! - **source**: the module's source tree, with a `Kbuild` or `Makefile`.
//! - **kernel-build**: the kernel build tree Buildroot's `kernel-build` output
//!   gives (`.config`, `Module.symvers`, `include/generated/utsrelease.h`,
//!   `scripts/`, the headers). It is read, never written: make builds the module
//!   in the step's own copy of the source.
//! - **toolchain**: the cross toolchain that kernel was built with, Buildroot's
//!   `toolchain` output (its whole host directory). The compiler is the target
//!   C compiler Buildroot records in its `share/buildroot/toolchainfile.cmake`,
//!   `bin/<cross-compile>gcc`, and its directory leads PATH. `modinfo` and
//!   `depmod` are its `sbin/` ones (host-kmod, which Buildroot builds with a
//!   kernel). No compiler or module tool comes from the host.
//! - **kernel-toolchain**: the board's declared kernel toolchain, `gcc` or
//!   `llvm` (`[kernel] toolchain`). The kernel build's `.config` must agree
//!   (`CONFIG_CC_IS_CLANG=y` is llvm), or the step refuses. With `llvm` the
//!   module is built as the kernel was: `LLVM=` the toolchain's
//!   `opt/llvm-prebuilt/bin/` (clang, lld and the LLVM tools; no
//!   `CROSS_COMPILE`). When the kernel has Rust (`CONFIG_RUST=y`), every Rust
//!   tool is the toolchain's too, named on make's command line and never
//!   left to PATH: `RUSTC=` and `HOSTRUSTC=` its `bin/rustc`, `BINDGEN=` its
//!   `bin/bindgen` (a module's Kbuild may run bindgen itself, as the
//!   kernel's `rust/Makefile` does), and `LIBCLANG_PATH` its
//!   `opt/llvm-prebuilt/lib`. PATH holds the toolchain and the system's
//!   directories, never a user's (`~/.cargo/bin`).
//! - **kernel-modules**: the image's kernel modules, Buildroot's `modules`
//!   artifact (`<release>/...`).
//! - **names**: the module names Kbuild builds (`obj-m`), space-separated,
//!   each giving `<name>.ko`.
//! - **host**: the Buildroot host: make, the job count and the CPU pin.
//!
//! The architecture (`ARCH`) is the one the kernel build's `.config` sets.
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
//! make runs under `bwrap` with the kernel build tree, the toolchain and the
//! module's source bound at fixed paths ([`FIXED`]): what the compiler
//! writes into a module (a header's `__FILE__` in a `WARN`, a Rust panic's
//! location) names those, never the store's location or the step's scratch
//! directory, so the same inputs make the same module in any store. Without
//! a `bwrap` (the host gives none) make runs where the trees are.
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
use gaia_types::{Artifact, ArtifactKind, BuildrootHost, Tree};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const NAME: &str = "gaia.kernel-module";
/// 2: the kernel toolchain, and modules built with LLVM. 3: modinfo,
/// depmod and the cross prefix from the toolchain, `ARCH` from the kernel's
/// `.config`, the source input named `source`. 4: a Rust kernel's bindgen,
/// host rustc and libclang named from the toolchain. 5: built at fixed
/// paths ([`FIXED`]), the same module from any store.
const KEY_VERSION: u32 = 5;
/// Where make sees the trees: the kernel build tree, the toolchain and the
/// module's source, each bound under this in a mount namespace of make's own
/// (so builds at once each see their own).
pub const FIXED: &str = "/dev/shm/gaia-kernel-module";
/// Where the toolchain tree holds the pinned LLVM (Gaia's Buildroot patch
/// 0006, `host-llvm-prebuilt`).
const LLVM_BIN: &str = "opt/llvm-prebuilt/bin";
/// Where it holds that LLVM's libclang, which bindgen loads.
const LLVM_LIB: &str = "opt/llvm-prebuilt/lib";
/// The directories every tool's PATH ends with, after the toolchain's `bin`.
const BASE_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
/// Fixed build stamps, so the same source gives the same module bytes.
const BUILD_TIMESTAMP: &str = "Thu Jan  1 00:00:00 UTC 1970";
const BUILD_USER: &str = "gaia";
const BUILD_HOST: &str = "gaia";
/// The step's output directory, in the scratch space.
const OUTPUT: &str = "modules";

/// What a module build runs: the Buildroot host's make, taskset, CPU pin and
/// jobs, and, from the kernel build and its toolchain, the architecture, the
/// cross prefix (empty with LLVM), modinfo and depmod.
struct KernelModuleHost {
    make: String,
    modinfo: String,
    depmod: String,
    arch: String,
    cross_compile: String,
    taskset: String,
    cpus: String,
    jobs: u32,
    /// bubblewrap, which runs make at [`FIXED`]; empty: none.
    bwrap: String,
}

/// Where Buildroot records its toolchain's target compiler.
const TOOLCHAIN_FILE: &str = "share/buildroot/toolchainfile.cmake";

/// Builds the modules `names` from `module`, against the kernel build tree
/// `kernel-build` and the `toolchain` it was built with, and places them in a
/// copy of the image's `kernel-modules` with depmod's index. See the module docs.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
#[node(
    id = "gaia.kernel-module",
    inputs(
        store,
        source,
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
    source: Tree,
    kernel_build: Tree,
    toolchain: Tree,
    kernel_toolchain: String,
    kernel_modules: Artifact,
    names: String,
    host: &BuildrootHost,
    cx: &Ctx,
) -> Result<Artifact, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |why: String| StepError::body(NAME, why);
    let names: Vec<String> = names.split_whitespace().map(str::to_string).collect();
    check(&names, host).map_err(fail)?;
    let (src, src_digest) = in_store(&store, &source, "the module source").map_err(fail)?;
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
    let cross_compile = match compile {
        module::Compile::Gcc => {
            let file = tool_dir.join(TOOLCHAIN_FILE);
            let text = fs::read_to_string(&file).map_err(|error| {
                fail(format!(
                    "the toolchain is not a Buildroot one (no {TOOLCHAIN_FILE}): {error}"
                ))
            })?;
            module::cross_prefix(&text).map_err(fail)?
        }
        module::Compile::Llvm { .. } => String::new(),
    };
    let host = KernelModuleHost {
        make: host.make.clone(),
        modinfo: kmod_tool(&tool_dir, "modinfo").map_err(fail)?,
        depmod: kmod_tool(&tool_dir, "depmod").map_err(fail)?,
        arch: module::arch_of(&config).map_err(fail)?.to_string(),
        cross_compile,
        taskset: host.taskset.clone(),
        cpus: host.cpus.clone(),
        jobs: host.jobs,
        bwrap: host.bwrap.clone(),
    };
    let bin = tool_dir.join("bin");
    let llvm = tool_dir.join(LLVM_BIN);
    let (cc, rust) = match compile {
        module::Compile::Gcc => (bin.join(format!("{}gcc", host.cross_compile)), None),
        module::Compile::Llvm { rust } => (
            llvm.join("clang"),
            rust.then(|| RustTools {
                rustc: bin.join("rustc"),
                bindgen: bin.join("bindgen"),
                libclang: tool_dir.join(LLVM_LIB),
            }),
        ),
    };
    if !cc.is_file() {
        return Err(fail(format!(
            "the toolchain has no compiler {} (kernel toolchain {kernel_toolchain}, cross-compile {:?})",
            cc.display(),
            host.cross_compile
        )));
    }
    if let Some(rust) = &rust {
        rust.check().map_err(fail)?;
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
    let (rustc_version, bindgen_version) = match &rust {
        Some(rust) => (
            tool_version(&path_string(&rust.rustc), &["--version"]).map_err(fail)?,
            tool_version(&path_string(&rust.bindgen), &["--version"]).map_err(fail)?,
        ),
        None => (Vec::new(), Vec::new()),
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
        .config("tool bindgen", bindgen_version)
        .config("tool modinfo", modinfo)
        .config("tool depmod", depmod)
        .input("source", src_digest)
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
                &tool_dir,
                &modules_dir,
                &Compiler {
                    cc: &cc,
                    bin: &bin,
                    llvm: matches!(compile, module::Compile::Llvm { .. }).then_some(llvm.as_path()),
                    rust: rust.as_ref(),
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
/// For a kernel built with LLVM, the LLVM `bin` (`LLVM=`), and the Rust
/// tools when it has Rust.
struct Compiler<'a> {
    cc: &'a Path,
    bin: &'a Path,
    llvm: Option<&'a Path>,
    rust: Option<&'a RustTools>,
}

/// A Rust kernel's tools, all the toolchain's: rustc (for the module and
/// for the host's procedural macros), bindgen, and the libclang it loads.
struct RustTools {
    rustc: PathBuf,
    bindgen: PathBuf,
    libclang: PathBuf,
}

impl RustTools {
    /// Each is in the toolchain: a Rust kernel's module is not built with a
    /// tool from anywhere else.
    fn check(&self) -> Result<(), String> {
        for (what, path) in [("rustc", &self.rustc), ("bindgen", &self.bindgen)] {
            if !path.is_file() {
                return Err(format!(
                    "the kernel has Rust, and the toolchain has no {what} {}",
                    path.display()
                ));
            }
        }
        let has_libclang = fs::read_dir(&self.libclang).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("libclang.so")
            })
        });
        if !has_libclang {
            return Err(format!(
                "the kernel has Rust, and the toolchain has no libclang in {}",
                self.libclang.display()
            ));
        }
        Ok(())
    }
}

fn check(names: &[String], host: &BuildrootHost) -> Result<(), String> {
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
    let mut tools = vec![("make", &host.make)];
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

/// The toolchain's `sbin/<name>` (host-kmod's link), or `bin/<name>`.
fn kmod_tool(toolchain: &Path, name: &str) -> Result<String, String> {
    ["sbin", "bin"]
        .iter()
        .map(|dir| toolchain.join(dir).join(name))
        .find(|path| path.is_file())
        .map(|path| path_string(&path))
        .ok_or_else(|| {
            format!("the toolchain has no {name} in sbin/ or bin/ (Buildroot builds host-kmod with a kernel)")
        })
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
    toolchain: &Path,
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

    let places = Places::new(host, build, toolchain, &source);
    let seen = SeenTools::new(compiler, toolchain, &places.toolchain);
    work.run(&make_tool(host, &places, &seen.compiler()))?;
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

/// The trees a module build reads and writes, where they are and where make
/// sees them: under [`FIXED`] with a `bwrap`, else where they are.
struct Places {
    /// (where it is, where make sees it), for the kernel build tree, the
    /// toolchain and the module's source.
    kernel: (PathBuf, PathBuf),
    toolchain: PathBuf,
    real_toolchain: PathBuf,
    module: (PathBuf, PathBuf),
}

impl Places {
    fn new(host: &KernelModuleHost, build: &Path, toolchain: &Path, source: &Path) -> Self {
        let seen = |name: &str, real: &Path| {
            if host.bwrap.is_empty() {
                real.to_path_buf()
            } else {
                Path::new(FIXED).join(name)
            }
        };
        Self {
            kernel: (build.to_path_buf(), seen("kernel", build)),
            toolchain: seen("toolchain", toolchain),
            real_toolchain: toolchain.to_path_buf(),
            module: (source.to_path_buf(), seen("module", source)),
        }
    }
}

/// The compiler's paths as make sees them: under the toolchain's place.
struct SeenTools {
    cc: PathBuf,
    bin: PathBuf,
    llvm: Option<PathBuf>,
    rust: Option<RustTools>,
}

impl SeenTools {
    fn new(compiler: &Compiler<'_>, real: &Path, seen: &Path) -> Self {
        let at = |path: &Path| match path.strip_prefix(real) {
            Ok(rest) => seen.join(rest),
            Err(_) => path.to_path_buf(),
        };
        Self {
            cc: at(compiler.cc),
            bin: at(compiler.bin),
            llvm: compiler.llvm.map(at),
            rust: compiler.rust.map(|rust| RustTools {
                rustc: at(&rust.rustc),
                bindgen: at(&rust.bindgen),
                libclang: at(&rust.libclang),
            }),
        }
    }

    fn compiler(&self) -> Compiler<'_> {
        Compiler {
            cc: &self.cc,
            bin: &self.bin,
            llvm: self.llvm.as_deref(),
            rust: self.rust.as_ref(),
        }
    }
}

/// make, run for the module against the kernel build tree, with the
/// toolchain's compiler, as `places` says make sees them. With a `bwrap`,
/// under it: the host's root as it is, the kernel build tree and the
/// toolchain bound read-only and the module's source read-write at
/// [`FIXED`]. With a CPU list, under `taskset`.
fn make_tool(host: &KernelModuleHost, places: &Places, compiler: &Compiler<'_>) -> Tool {
    let (kernel, module) = (&places.kernel.1, &places.module.1);
    let mut words: Vec<String> = Vec::new();
    if !host.bwrap.is_empty() {
        // The mount points: empty, and never removed; each build binds over
        // them in its own namespace.
        for dir in [kernel, &places.toolchain, module] {
            let _ = fs::create_dir_all(dir);
        }
        words.extend([
            host.bwrap.clone(),
            "--dev-bind".into(),
            "/".into(),
            "/".into(),
            "--ro-bind".into(),
            path_string(&places.kernel.0),
            path_string(kernel),
            "--ro-bind".into(),
            path_string(&places.real_toolchain),
            path_string(&places.toolchain),
            "--bind".into(),
            path_string(&places.module.0),
            path_string(module),
            "--chdir".into(),
            path_string(module),
            "--die-with-parent".into(),
            "--".into(),
        ]);
    }
    if !host.cpus.is_empty() {
        words.extend([host.taskset.clone(), "-c".into(), host.cpus.clone()]);
    }
    words.push(host.make.clone());
    let mut words = words.into_iter();
    let mut tool = Tool::new(words.next().unwrap_or_default())
        .args(words)
        .cwd(&places.module.0)
        .arg("-C")
        .arg(kernel)
        .arg(format!("M={}", module.display()))
        .arg(format!("ARCH={}", host.arch))
        .arg(format!("-j{}", host.jobs));
    for arg in compile_args(host, compiler) {
        tool = tool.arg(arg);
    }
    if let Some(rust) = compiler.rust {
        tool = tool.env("LIBCLANG_PATH", path_string(&rust.libclang));
    }
    tool.arg("modules")
        .env("PATH", path_env(compiler))
        .env("LC_ALL", "C")
        .env("HOME", module.display().to_string())
        .env("KBUILD_BUILD_TIMESTAMP", BUILD_TIMESTAMP)
        .env("KBUILD_BUILD_USER", BUILD_USER)
        .env("KBUILD_BUILD_HOST", BUILD_HOST)
}

/// make's compiler words: as the kernel was built. gcc: `CC=` the
/// toolchain's and its `CROSS_COMPILE`. LLVM: `LLVM=` its `bin/` (clang takes
/// the target from `ARCH`), and for a kernel with Rust `RUSTC=`,
/// `HOSTRUSTC=` and `BINDGEN=`, so make's own defaults (`rustc`,
/// `bindgen`, looked up on PATH) are never used.
fn compile_args(host: &KernelModuleHost, compiler: &Compiler<'_>) -> Vec<String> {
    let Some(llvm) = compiler.llvm else {
        let mut args = vec![format!("CC={}", path_string(compiler.cc))];
        if !host.cross_compile.is_empty() {
            args.push(format!("CROSS_COMPILE={}", host.cross_compile));
        }
        return args;
    };
    let mut args = vec![format!("LLVM={}/", path_string(llvm))];
    if let Some(rust) = compiler.rust {
        args.push(format!("RUSTC={}", path_string(&rust.rustc)));
        args.push(format!("HOSTRUSTC={}", path_string(&rust.rustc)));
        args.push(format!("BINDGEN={}", path_string(&rust.bindgen)));
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
            bwrap: String::new(),
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
            rust: None,
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
            rust: None,
        };
        assert_eq!(
            compile_args(&host(), &c),
            ["LLVM=/t/opt/llvm-prebuilt/bin/"]
        );
        assert_eq!(
            path_env(&c),
            format!("/t/opt/llvm-prebuilt/bin:/t/bin:{BASE_PATH}")
        );
        // The plant's mirror: a Rust kernel's every Rust tool is named,
        // the toolchain's, and libclang is given; none is left to PATH.
        let tools = RustTools {
            rustc: "/t/bin/rustc".into(),
            bindgen: "/t/bin/bindgen".into(),
            libclang: "/t/opt/llvm-prebuilt/lib".into(),
        };
        let rust = Compiler {
            rust: Some(&tools),
            ..c
        };
        assert_eq!(
            compile_args(&host(), &rust),
            [
                "LLVM=/t/opt/llvm-prebuilt/bin/",
                "RUSTC=/t/bin/rustc",
                "HOSTRUSTC=/t/bin/rustc",
                "BINDGEN=/t/bin/bindgen"
            ]
        );
        let make = format!("{:?}", make_tool(&host(), &bare(), &rust));
        assert!(
            make.contains(r#"("LIBCLANG_PATH", "/t/opt/llvm-prebuilt/lib")"#),
            "{make}"
        );
        assert!(!make.contains(".cargo"), "{make}");
        let gcc_make = format!("{:?}", make_tool(&host(), &bare(), &gcc));
        assert!(
            !gcc_make.contains("LIBCLANG_PATH") && !gcc_make.contains("BINDGEN"),
            "{gcc_make}"
        );
    }

    /// The places of a build with no `bwrap`: where the trees are.
    fn bare() -> Places {
        Places::new(&host(), Path::new("/b"), Path::new("/t"), Path::new("/s"))
    }

    /// With a `bwrap`, make sees the kernel build tree, the toolchain and
    /// the module's source at the fixed paths, whatever store they are in:
    /// the trees bound there (the kernel and toolchain read-only), make
    /// pointed there, and every compiler path the toolchain's under its
    /// fixed place.
    #[test]
    fn make_sees_the_trees_at_the_fixed_paths() {
        let mut sandboxed = host();
        sandboxed.bwrap = "/usr/bin/bwrap".into();
        let places = Places::new(
            &sandboxed,
            Path::new("/store/objects/tree/ab/kernel"),
            Path::new("/store/objects/tree/cd/toolchain"),
            Path::new("/store/scratch/7/src"),
        );
        let real = Path::new("/store/objects/tree/cd/toolchain");
        let tools = RustTools {
            rustc: real.join("bin/rustc"),
            bindgen: real.join("bin/bindgen"),
            libclang: real.join("opt/llvm-prebuilt/lib"),
        };
        let (cc, bin, llvm) = (
            real.join("opt/llvm-prebuilt/bin/clang"),
            real.join("bin"),
            real.join("opt/llvm-prebuilt/bin"),
        );
        let compiler = Compiler {
            cc: &cc,
            bin: &bin,
            llvm: Some(&llvm),
            rust: Some(&tools),
        };
        let seen = SeenTools::new(&compiler, real, &places.toolchain);
        let make = format!("{:?}", make_tool(&sandboxed, &places, &seen.compiler()));
        let words: Vec<String> = make_words(&make);
        let has = |pair: [&str; 3]| {
            words
                .windows(3)
                .any(|w| w[0] == pair[0] && w[1] == pair[1] && w[2] == pair[2])
        };
        assert_eq!(words[0], "/usr/bin/bwrap", "{make}");
        assert!(
            has([
                "--ro-bind",
                "/store/objects/tree/ab/kernel",
                "/dev/shm/gaia-kernel-module/kernel"
            ]),
            "{make}"
        );
        assert!(
            has([
                "--ro-bind",
                "/store/objects/tree/cd/toolchain",
                "/dev/shm/gaia-kernel-module/toolchain"
            ]),
            "{make}"
        );
        assert!(
            has([
                "--bind",
                "/store/scratch/7/src",
                "/dev/shm/gaia-kernel-module/module"
            ]),
            "{make}"
        );
        let after: Vec<&String> = words.iter().skip_while(|word| *word != "--").collect();
        for word in [
            "/dev/shm/gaia-kernel-module/kernel",
            "M=/dev/shm/gaia-kernel-module/module",
            "LLVM=/dev/shm/gaia-kernel-module/toolchain/opt/llvm-prebuilt/bin/",
            "RUSTC=/dev/shm/gaia-kernel-module/toolchain/bin/rustc",
            "BINDGEN=/dev/shm/gaia-kernel-module/toolchain/bin/bindgen",
        ] {
            assert!(after.iter().any(|w| *w == word), "{word}: {make}");
        }
        // What make runs with names no store path.
        for word in &after {
            assert!(!word.contains("/store/"), "{word}: {make}");
        }
        assert!(
            make.contains(r#"("LIBCLANG_PATH", "/dev/shm/gaia-kernel-module/toolchain/opt/llvm-prebuilt/lib")"#),
            "{make}"
        );
        assert!(
            make.contains(r#"("PATH", "/dev/shm/gaia-kernel-module/toolchain/opt/llvm-prebuilt/bin:/dev/shm/gaia-kernel-module/toolchain/bin:"#),
            "{make}"
        );
    }

    /// The program and arguments of a [`Tool`]'s debug form, in order.
    fn make_words(debug: &str) -> Vec<String> {
        let mut words = Vec::new();
        let program = debug
            .split("program: \"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or_default();
        words.push(program.to_string());
        if let Some(args) = debug
            .split("args: [")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
        {
            words.extend(
                args.split(", ")
                    .map(|word| word.trim_matches('"').to_string())
                    .filter(|word| !word.is_empty()),
            );
        }
        words
    }

    /// A toolchain missing any Rust tool is refused by name.
    #[test]
    fn a_rust_kernel_without_a_toolchain_rust_tool_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let t = dir.path();
        let tools = RustTools {
            rustc: t.join("bin/rustc"),
            bindgen: t.join("bin/bindgen"),
            libclang: t.join("opt/llvm-prebuilt/lib"),
        };
        fs::create_dir_all(t.join("bin")).expect("bin");
        fs::create_dir_all(&tools.libclang).expect("lib");
        let why = tools.check().expect_err("nothing");
        assert!(why.contains("has no rustc"), "{why}");
        fs::write(&tools.rustc, "").expect("rustc");
        let why = tools.check().expect_err("no bindgen");
        assert!(why.contains("has no bindgen"), "{why}");
        fs::write(&tools.bindgen, "").expect("bindgen");
        let why = tools.check().expect_err("no libclang");
        assert!(why.contains("has no libclang"), "{why}");
        fs::write(tools.libclang.join("libclang.so.23.1"), "").expect("libclang");
        tools.check().expect("all three");
    }
}
