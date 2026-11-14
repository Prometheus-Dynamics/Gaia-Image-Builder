//! `gaia.os.buildroot`: a root filesystem and boot files built by Buildroot.
//!
//! Inputs: the Buildroot source (a stored tree); `os`, the config
//! ([`OsConfig`]): fragments, text from descriptions applied in order over
//! Buildroot's own `allnoconfig`, the names of the input trees, packages
//! built from one of them (`<PACKAGE>_OVERRIDE_SRCDIR`) and those given as
//! `BR2_EXTERNAL`; the input trees, on ports `inputs0`, `inputs1` and so
//! on; and the host's make, CPU pinning, job count and download cache. A
//! fragment names an input tree as `@name@`, replaced with its path when
//! the config is written, so the key does not change with the store's
//! place. Buildroot runs in a fresh output directory in
//! the step's scratch space, and its outputs are moved into the store, one
//! artifact each:
//!
//! - **rootfs**: Buildroot's target tree, with the owners and modes
//!   Buildroot gives a filesystem image as the tree's entries (see `perms`).
//! - **kernel**, **device-trees**, **bootloader**, **images**: the files of
//!   its `images/` directory, sorted by name (see `out`).
//! - **modules**: the kernel modules inside the target (`lib/modules`).
//! - **config**: the resolved `.config`, as `buildroot.config`.
//! - **bootloader**: the bootloader's images, and `u-boot.config`, U-Boot's
//!   final configuration, when the build made U-Boot.
//! - **kernel-build**: when the build has a kernel, the tree an external module
//!   build needs after `modules_prepare` (see `out::copy_kernel_build`). Empty
//!   otherwise.
//! - **toolchain**: when the build has a kernel, Buildroot's host directory: the
//!   cross compiler, its sysroot and the host tools. Empty otherwise.
//!
//! Outputs are added, never changed: a build with no kernel makes the same
//! files and the same key, and two empty trees more.
//!
//! The key holds the source's digest, each input tree's digest by name, the
//! fragments' text as written, the overrides, the externals and the make
//! program. The download cache, job count and
//! CPU list change how fast it runs, not what it makes, so they are not in
//! the key.
//!
//! Progress: Buildroot's `Installing to target` lines are counted as they
//! print, but the step kit reports progress only between tool runs, so the
//! count reaches Daedalus after `make` ends. See the PR for that gap.

mod config;
mod contract;
mod host;
mod out;
mod perms;
mod progress;

use daedalus::prelude::*;
use daedalus_cpu_author::FanIn;
use gaia_step::{Ctx, Digest, KeyBuilder, Step, StepError, Store, Stream, Tool, Work};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub(crate) const NAME: &str = "gaia.os.buildroot";
/// 2: the root filesystem's owners and modes, from Buildroot's own tar.
/// (The check that every config line was taken changes no output, so no
/// new version.) 3: the host tools another step built, as an input.
const KEY_VERSION: u32 = 3;
/// The environment every make runs with. Nothing else is inherited.
pub(crate) const PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// The step's outputs, by name and by path inside its directory. The last two
/// are empty when the build has no kernel.
const OUTPUTS: [(&str, &str); 10] = [
    ("rootfs-entries", "rootfs-entries.json"),
    ("rootfs", "rootfs"),
    ("kernel", "kernel"),
    ("device-trees", "device-trees"),
    ("modules", "modules"),
    ("bootloader", "bootloader"),
    ("images", "images"),
    ("config", "config/buildroot.config"),
    ("kernel-build", "kernel-build"),
    ("toolchain", "toolchain"),
];

pub use gaia_types::{BuildrootHost, ConfigFragment, OsConfig};
pub use host::{build_host, host_goals, os_buildroot_host};

/// The node's outputs, in port order.
pub type Outputs = (
    Tree,
    Artifact,
    Artifact,
    Artifact,
    Artifact,
    Artifact,
    Artifact,
    Tree,
    Tree,
);

/// Builds the Buildroot tree `source` with `config` and its input trees,
/// and gives its root filesystem and the artifacts described in the module
/// docs, in that order.
// The nine outputs are one tuple: the node macro reads it as the output ports.
#[allow(clippy::type_complexity)]
#[node(
    id = "gaia.os.buildroot",
    inputs(store, source, os, inputs, seed, host),
    outputs(
        rootfs,
        kernel,
        device_trees,
        modules,
        bootloader,
        images,
        config,
        kernel_build,
        toolchain
    )
)]
pub fn os_buildroot(
    store: &str,
    source: Tree,
    os: &OsConfig,
    inputs: FanIn<'_, Tree>,
    seed: Tree,
    host: &BuildrootHost,
    cx: &Ctx,
) -> Result<
    (
        Tree,
        Artifact,
        Artifact,
        Artifact,
        Artifact,
        Artifact,
        Artifact,
        Tree,
        Tree,
    ),
    StepError,
> {
    let inputs: Vec<Tree> = inputs.iter().map(|tree| (*tree).clone()).collect();
    build_os(store, source, os, &inputs, &seed, host, cx)
}

/// What a Buildroot run is given, worked out from the config and the input
/// trees: the same for the OS step and for the host tools' step.
pub(crate) struct Prepared {
    /// The Buildroot source in the store.
    pub src: PathBuf,
    pub src_digest: Digest,
    /// Each input tree: its name, its path in the store, its digest.
    pub dirs: Vec<(String, PathBuf, Digest)>,
    /// `NAME=value` arguments for make: externals and source overrides.
    pub vars: Vec<String>,
    /// The fragments with each `@name@` replaced by its tree's path.
    pub fragments: Vec<ConfigFragment>,
    /// The contracts the kernel's final config must meet, each with the
    /// path of its file in the store.
    pub contracts: Vec<(gaia_types::KernelContract, PathBuf)>,
}

impl Prepared {
    /// The key parts every step over this config has: the source, each
    /// input tree, the fragments as written, the overrides, the externals
    /// and the make program.
    pub fn key(
        &self,
        name: &str,
        version: u32,
        config: &OsConfig,
        host: &BuildrootHost,
    ) -> KeyBuilder {
        let mut key = KeyBuilder::new(name, version)
            .input("source", self.src_digest)
            .config("base", "allnoconfig")
            .config("make", &host.make);
        for (name, _, digest) in &self.dirs {
            key = key.input(&format!("input.{name}"), *digest);
        }
        for fragment in &config.fragments {
            key = key.config(&format!("fragment.{}", fragment.name), &fragment.text);
        }
        for o in &config.overrides {
            key = key.config(&format!("override.{}", o.package), &o.input);
        }
        key = key.config("externals", config.externals.join("\n"));
        // Only when there is one: a build without a contract keeps its key.
        for contract in &config.kernel_contracts {
            key = key.config(
                &format!("kernel-contract.{}", contract.input),
                format!(
                    "{}\n{}\n{}\n{}\n{}",
                    contract.path,
                    contract.board,
                    contract.kind,
                    contract.firmware,
                    contract.storage_drivers.join(" ")
                ),
            );
        }
        key
    }
}

pub(crate) fn prepare(
    store: &Store,
    source: &Tree,
    config: &OsConfig,
    inputs: &[Tree],
    host: &BuildrootHost,
) -> Result<Prepared, String> {
    check(config, inputs.len(), host)?;
    let (src, src_digest) = in_store(store, source, "the Buildroot source")?;
    let mut dirs = Vec::with_capacity(inputs.len());
    for (name, tree) in config.inputs.iter().zip(inputs) {
        let (dir, digest) = in_store(store, tree, &format!("input {name}"))?;
        dirs.push((name.clone(), dir, digest));
    }
    let path_of = |name: &str| -> PathBuf {
        dirs.iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, dir, _)| dir.clone())
            .unwrap_or_default()
    };
    let mut vars = Vec::new();
    if !config.externals.is_empty() {
        let joined = config
            .externals
            .iter()
            .map(|name| path_of(name).display().to_string())
            .collect::<Vec<_>>()
            .join(":");
        vars.push(format!("BR2_EXTERNAL={joined}"));
    }
    for o in &config.overrides {
        vars.push(format!(
            "{}_OVERRIDE_SRCDIR={}",
            o.package,
            path_of(&o.input).display()
        ));
    }
    let paths: Vec<(String, PathBuf)> = dirs
        .iter()
        .map(|(name, dir, _)| (name.clone(), dir.clone()))
        .collect();
    let fragments = config
        .fragments
        .iter()
        .map(|f| {
            Ok(ConfigFragment {
                name: f.name.clone(),
                text: config::substitute(&f.text, &paths)
                    .map_err(|why| format!("fragment {:?}: {why}", f.name))?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let contracts = config
        .kernel_contracts
        .iter()
        .map(|contract| {
            let file = path_of(&contract.input).join(&contract.path);
            (contract.clone(), file)
        })
        .collect();
    Ok(Prepared {
        src,
        src_digest,
        dirs,
        vars,
        fragments,
        contracts,
    })
}

/// An empty seed: no host tools from another step, so the build makes its
/// own. For a build that stands alone.
pub fn no_seed(store: &str) -> Result<Tree, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let scratch = store.scratch().map_err(StepError::store(NAME))?;
    fs::create_dir(scratch.path().join("seed"))
        .map_err(|e| StepError::body(NAME, format!("an empty seed: {e}")))?;
    let made = scratch
        .commit(&[("seed", Path::new("seed"))])
        .map_err(StepError::store(NAME))?;
    Ok(Tree {
        object: made["seed"].into(),
        entries: Vec::new(),
    })
}

/// The node's body, with the input trees as a slice. `seed` is the host
/// tools another step built (see [`host`]); an empty tree is none.
pub fn build_os(
    store: &str,
    source: Tree,
    config: &OsConfig,
    inputs: &[Tree],
    seed: &Tree,
    host: &BuildrootHost,
    cx: &Ctx,
) -> Result<Outputs, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |why: String| StepError::body(NAME, why);
    let prepared = prepare(&store, &source, config, inputs, host).map_err(fail)?;
    let (seed_dir, seed_digest) = in_store(&store, seed, "the host tools").map_err(fail)?;
    let key = prepared
        .key(NAME, KEY_VERSION, config, host)
        .input("seed", seed_digest);
    let outputs: Vec<(&str, &Path)> = OUTPUTS
        .iter()
        .map(|(name, path)| (*name, Path::new(*path)))
        .collect();
    let made = Step::new(&store, cx, NAME, key.finish())
        .look_up_or_make(&outputs, |work| build(work, &prepared, &seed_dir, host))?;
    // Read back on a hit too: the entries are part of what the step made.
    let listed = fs::read(store.path(&made.outputs["rootfs-entries"]))
        .map_err(|e| StepError::body(NAME, format!("the root filesystem's entries: {e}")))?;
    let rootfs_entries: Vec<gaia_types::FileEntry> = serde_json::from_slice(&listed)
        .map_err(|e| StepError::body(NAME, format!("the root filesystem's entries: {e}")))?;
    let artifact = |name: &str, kind: ArtifactKind| Artifact {
        kind,
        object: made.outputs[name].into(),
    };
    let tree = |name: &str| Tree {
        object: made.outputs[name].into(),
        entries: Vec::new(),
    };
    Ok((
        Tree {
            object: made.outputs["rootfs"].into(),
            entries: rootfs_entries,
        },
        artifact("kernel", ArtifactKind::Kernel),
        artifact("device-trees", ArtifactKind::DeviceTree),
        artifact("modules", ArtifactKind::parse("modules")),
        artifact("bootloader", ArtifactKind::parse("bootloader")),
        artifact("images", ArtifactKind::parse("image")),
        artifact("config", ArtifactKind::Config),
        tree("kernel-build"),
        tree("toolchain"),
    ))
}

fn check(config: &OsConfig, inputs: usize, host: &BuildrootHost) -> Result<(), String> {
    config.check()?;
    if inputs != config.inputs.len() {
        return Err(format!(
            "the config names {} input trees, and {inputs} are given",
            config.inputs.len()
        ));
    }
    let fragments = &config.fragments;
    if host.jobs == 0 {
        return Err("host jobs must be at least 1".into());
    }
    for (i, fragment) in fragments.iter().enumerate() {
        if fragments[..i]
            .iter()
            .any(|other| other.name == fragment.name)
        {
            return Err(format!("fragment {:?} is named twice", fragment.name));
        }
    }
    for (what, path) in [("make", &host.make), ("download dir", &host.download_dir)] {
        if !Path::new(path).is_absolute() {
            return Err(format!("{what} {path:?} is not an absolute path"));
        }
    }
    Ok(())
}

/// A tree in the store: its path there and its digest.
pub(crate) fn in_store(
    store: &Store,
    tree: &Tree,
    what: &str,
) -> Result<(PathBuf, Digest), String> {
    let object = tree
        .object
        .object_ref()
        .ok_or_else(|| format!("{what} is not in the store"))?;
    Ok((store.path(&object), object.digest))
}

/// The body of the step: configure, resolve, make, then sort the outputs
/// out of Buildroot's output directory.
/// Buildroot's config in `out`: its `allnoconfig`, the fragments over it,
/// resolved, with every line checked to have been taken. Returns the
/// resolved text.
pub(crate) fn configure(
    step: &'static str,
    work: &mut Work<'_>,
    prepared: &Prepared,
    out: &Path,
    host: &BuildrootHost,
) -> Result<String, StepError> {
    let fail = |why: String| StepError::body(step, why);
    let io_fail = |what: &'static str| {
        move |error: io::Error| StepError::body(step, format!("{what}: {error}"))
    };
    let (src, vars) = (&prepared.src, &prepared.vars);
    work.run(&make_tool(host, src, out, vars, &["allnoconfig"]))?;
    let config = out.join(".config");
    let base = fs::read_to_string(&config).map_err(io_fail("the base config"))?;
    // Last, so no description turns it off: the tar is where the root
    // filesystem's owners and modes are read from.
    let mut fragments = prepared.fragments.clone();
    fragments.push(ConfigFragment {
        name: "gaia.permissions".into(),
        text: perms::FRAGMENT.into(),
    });
    let merged = config::merge(&base, &fragments).map_err(fail)?;
    fs::write(&config, merged).map_err(io_fail("the merged config"))?;
    work.run(&make_tool(host, src, out, vars, &["olddefconfig"]))?;
    // What Buildroot resolved must be what was asked for.
    let resolved = fs::read_to_string(&config).map_err(io_fail("the resolved config"))?;
    let not_taken = config::not_taken(&resolved, &fragments);
    if !not_taken.is_empty() {
        return Err(fail(format!(
            "Buildroot did not take {} config line{} as written:\n  {}",
            not_taken.len(),
            if not_taken.len() == 1 { "" } else { "s" },
            not_taken.join("\n  ")
        )));
    }
    Ok(resolved)
}

fn build(
    work: &mut Work<'_>,
    prepared: &Prepared,
    seed: &Path,
    host: &BuildrootHost,
) -> Result<(), StepError> {
    // One build at a time: each has the whole job count.
    let _turn = turn();
    let fail = |why: String| StepError::body(NAME, why);
    let io_fail = |what: &'static str| {
        move |error: io::Error| StepError::body(NAME, format!("{what}: {error}"))
    };
    let (src, vars) = (&prepared.src, &prepared.vars);
    let dir = work.dir().to_path_buf();
    let out = dir.join("out");
    fs::create_dir(&out).map_err(io_fail("output directory"))?;
    // The host tools another step built, before Buildroot looks: it finds
    // them installed and builds none of them again.
    host::plant(seed, &out).map_err(io_fail("the host tools"))?;
    let config = out.join(".config");
    let resolved = configure(NAME, work, prepared, &out, host)?;

    // The kernel's and the bootloader's own configs, before anything
    // compiles: every line their fragment files ask for must be in the
    // package's final `.config`.
    for (package, enabled, files_option) in [
        (
            "linux",
            "BR2_LINUX_KERNEL=y",
            "BR2_LINUX_KERNEL_CONFIG_FRAGMENT_FILES",
        ),
        (
            "uboot",
            "BR2_TARGET_UBOOT=y",
            "BR2_TARGET_UBOOT_CONFIG_FRAGMENT_FILES",
        ),
    ] {
        let files = config::string_option(&resolved, files_option).unwrap_or_default();
        let has = resolved.lines().any(|line| line.trim() == enabled);
        let contracts: &[_] = if package == "linux" {
            &prepared.contracts
        } else {
            &[]
        };
        if !contracts.is_empty() && !has {
            return Err(fail(format!(
                "{} states a kernel contract ({}), and this build has no kernel",
                contracts[0].0.asked_by, contracts[0].0.path
            )));
        }
        // A complete configuration the kernel starts from: held to the
        // final config as the first of its fragments.
        let complete = if package == "linux" {
            config::string_option(&resolved, "BR2_LINUX_KERNEL_CUSTOM_CONFIG_FILE")
                .filter(|file| !file.is_empty())
        } else {
            None
        };
        if !has || (files.is_empty() && contracts.is_empty() && complete.is_none()) {
            continue;
        }
        let goal = format!("{package}-configure");
        work.run(&make_tool(host, src, &out, vars, &[goal.as_str()]))?;
        let mut fragments = Vec::new();
        for file in complete.into_iter().chain(files.split_whitespace()) {
            let text = fs::read_to_string(file)
                .map_err(|e| fail(format!("{package} config fragment {file}: {e}")))?;
            let name = Path::new(file)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            fragments.push((name, text));
        }
        let Some(final_config) = out::package_config(&out.join("build"), package)
            .map_err(io_fail("the package's config"))?
        else {
            return Err(fail(format!(
                "{package} was configured, and no .config was found under build/{package}-*"
            )));
        };
        let text = fs::read_to_string(&final_config).map_err(io_fail("the package's config"))?;
        let not_taken = config::kconfig_not_taken(&text, &fragments);
        if !not_taken.is_empty() {
            return Err(fail(format!(
                "{package}'s config did not take {} line{} of its fragments as written:\n  {}",
                not_taken.len(),
                if not_taken.len() == 1 { "" } else { "s" },
                not_taken.join("\n  ")
            )));
        }
        // What a boot path needs built in, by its owner's own list.
        for (contract, file) in contracts {
            let stated = fs::read_to_string(file)
                .map_err(|e| fail(format!("the kernel contract {}: {e}", contract.path)))?;
            let missing = contract::problems(&stated, &text, contract).map_err(fail)?;
            if !missing.is_empty() {
                return Err(fail(format!(
                    "the kernel for the board {} does not meet the contract of {}:\n  {}",
                    contract.board,
                    contract.asked_by,
                    missing.join("\n  ")
                )));
            }
        }
    }

    let shown = Arc::new(Mutex::new(String::new()));
    let sink = shown.clone();
    let order =
        make_tool(host, src, &out, vars, &["show-build-order"]).on_line(move |stream, line| {
            if stream == Stream::Stdout
                && let Ok(mut text) = sink.lock()
            {
                text.push_str(line);
                text.push('\n');
            }
        });
    work.run(&order)?;
    let listed = shown.lock().map(|text| text.clone()).unwrap_or_default();
    let targets = progress::target_packages(&listed);
    let total = u32::try_from(targets.len()).unwrap_or(u32::MAX);
    work.progress(0, total);

    let installed = Arc::new(Mutex::new(BTreeSet::new()));
    let sink = installed.clone();
    let build = make_tool(host, src, &out, vars, &[]).on_line(move |_, line| {
        if let Some(name) = progress::installed_to_target(line)
            && let Ok(mut set) = sink.lock()
        {
            set.insert(name.to_string());
        }
    });
    work.run(&build)?;
    let installed = installed.lock().map(|set| set.clone()).unwrap_or_default();
    work.progress(progress::done(&installed, &targets), total);

    // The owners and modes Buildroot gives the image, from the tar it packed
    // under fakeroot. The tar itself is not an output.
    let tar = out.join("images").join(perms::TAR);
    let entries = if tar.is_file() {
        let entries = perms::entries(&tar, &out.join("target"))
            .map_err(io_fail("the root filesystem's owners and modes"))?;
        fs::remove_file(&tar).map_err(io_fail("the root filesystem's tar"))?;
        entries
    } else {
        Vec::new()
    };
    let text = serde_json::to_vec(&entries).map_err(|e| fail(e.to_string()))?;
    fs::write(dir.join("rootfs-entries.json"), text).map_err(io_fail("the entries"))?;
    fs::rename(out.join("target"), dir.join("rootfs")).map_err(io_fail("the target tree"))?;
    out::copy_tree(&dir.join("rootfs/lib/modules"), &dir.join("modules"))
        .map_err(io_fail("the kernel modules"))?;
    out::sort_images(&out.join("images"), &dir).map_err(io_fail("the images"))?;
    // The bootloader's final configuration, beside what it made: what a
    // claim about a built U-Boot is read from, as the kernel's is from its
    // build tree.
    if let Some(final_config) =
        out::package_config(&out.join("build"), "uboot").map_err(io_fail("U-Boot's config"))?
    {
        fs::copy(&final_config, dir.join("bootloader/u-boot.config"))
            .map_err(io_fail("U-Boot's config"))?;
    }
    fs::create_dir(dir.join("config")).map_err(io_fail("the config directory"))?;
    fs::copy(&config, dir.join("config/buildroot.config")).map_err(io_fail("the config"))?;
    match out::kernel_build_source(&out.join("build")).map_err(io_fail("the kernel build"))? {
        Some(source) => {
            out::copy_kernel_build(&source, &dir.join("kernel-build"))
                .map_err(io_fail("the kernel build tree"))?;
            // Buildroot's host directory holds the cross compiler it built or
            // installed, with its sysroot. Nothing in it names the output path.
            fs::rename(out.join("host"), dir.join("toolchain"))
                .map_err(io_fail("the toolchain"))?;
        }
        None => {
            fs::create_dir(dir.join("kernel-build")).map_err(io_fail("the kernel build"))?;
            fs::create_dir(dir.join("toolchain")).map_err(io_fail("the toolchain"))?;
        }
    }
    Ok(())
}

/// One make run in `out`, for `goals` (none means Buildroot's default,
/// `all`). With a CPU list, make runs under `taskset`.
/// One Buildroot build at a time in this process. Each is given the whole
/// job count (`-j<jobs>`), so two at once would run twice the bound the
/// build was given; the host tools and the builds that start from them
/// take turns instead, and the bound is the bound.
static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// This build's turn, held until the guard is dropped.
pub(crate) fn turn() -> std::sync::MutexGuard<'static, ()> {
    TURN.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The make command for `goals`: its arguments, in order, for a test to
/// read.
pub fn make_arguments(
    host: &BuildrootHost,
    out: &Path,
    vars: &[String],
    goals: &[&str],
) -> Vec<String> {
    let mut args = Vec::new();
    if !host.cpus.is_empty() {
        args.extend([host.taskset.clone(), "-c".into(), host.cpus.clone()]);
    }
    args.push(host.make.clone());
    args.push("--no-print-directory".into());
    args.push(format!("-j{}", host.jobs));
    args.push(format!("O={}", out.display()));
    args.push(format!("BR2_DL_DIR={}", host.download_dir));
    args.extend(vars.iter().cloned());
    args.extend(goals.iter().map(|goal| (*goal).to_string()));
    args
}

pub(crate) fn make_tool(
    host: &BuildrootHost,
    src: &Path,
    out: &Path,
    vars: &[String],
    goals: &[&str],
) -> Tool {
    let mut arguments = make_arguments(host, out, vars, goals).into_iter();
    let program = arguments.next().unwrap_or_default();
    Tool::new(program)
        .cwd(src)
        .args(arguments)
        .env("PATH", PATH)
        .env("LC_ALL", "C")
        .env("HOME", out.display().to_string())
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 2] = [&OS_BUILDROOT_NODE, &host::OS_BUILDROOT_HOST_NODE];

#[cfg(test)]
mod bound_tests {
    use super::*;

    fn host(jobs: u32, cpus: &str) -> BuildrootHost {
        BuildrootHost {
            make: "/usr/bin/make".into(),
            taskset: "/usr/bin/taskset".into(),
            cpus: cpus.into(),
            jobs,
            download_dir: "/dl".into(),
            config_only: false,
        }
    }

    /// The job count the build was given is on make's command line, with
    /// the CPU pin before it when there is one.
    #[test]
    fn makes_command_line_has_the_job_count_it_was_given() {
        let args = make_arguments(&host(13, ""), Path::new("/out"), &[], &["world"]);
        assert_eq!(
            args,
            [
                "/usr/bin/make",
                "--no-print-directory",
                "-j13",
                "O=/out",
                "BR2_DL_DIR=/dl",
                "world"
            ]
        );
        let pinned = make_arguments(&host(4, "6-31"), Path::new("/out"), &["A=1".into()], &[]);
        assert_eq!(
            pinned[..6],
            [
                "/usr/bin/taskset",
                "-c",
                "6-31",
                "/usr/bin/make",
                "--no-print-directory",
                "-j4"
            ]
        );
        assert_eq!(pinned.iter().filter(|arg| arg.starts_with("-j")).count(), 1);
    }

    /// Builds take turns: each has the whole job count, so two at once
    /// would be twice the bound.
    #[test]
    fn two_builds_do_not_run_at_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (running, most) = (AtomicUsize::new(0), AtomicUsize::new(0));
        std::thread::scope(|scope| {
            for _ in 0..3 {
                scope.spawn(|| {
                    let _turn = turn();
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    most.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(60));
                    running.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        assert_eq!(most.load(Ordering::SeqCst), 1);
    }
}
