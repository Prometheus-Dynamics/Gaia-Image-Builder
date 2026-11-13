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

const NAME: &str = "gaia.os.buildroot";
/// 2: the root filesystem's owners and modes, from Buildroot's own tar.
/// (The check that every config line was taken changes no output, so no
/// new version.)
const KEY_VERSION: u32 = 2;
/// The environment every make runs with. Nothing else is inherited.
const PATH: &str = "/usr/local/bin:/usr/bin:/bin";

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
    inputs(store, source, os, inputs, host),
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
    build_os(store, source, os, &inputs, host, cx)
}

/// The node's body, with the input trees as a slice.
pub fn build_os(
    store: &str,
    source: Tree,
    config: &OsConfig,
    inputs: &[Tree],
    host: &BuildrootHost,
    cx: &Ctx,
) -> Result<Outputs, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |why: String| StepError::body(NAME, why);
    check(config, inputs.len(), host).map_err(fail)?;
    let (src, src_digest) = in_store(&store, &source, "the Buildroot source").map_err(fail)?;
    let mut key = KeyBuilder::new(NAME, KEY_VERSION)
        .input("source", src_digest)
        .config("base", "allnoconfig")
        .config("make", &host.make);
    let mut dirs = Vec::with_capacity(inputs.len());
    for (name, tree) in config.inputs.iter().zip(inputs) {
        let (dir, digest) = in_store(&store, tree, &format!("input {name}")).map_err(fail)?;
        key = key.input(&format!("input.{name}"), digest);
        dirs.push((name.clone(), dir));
    }
    for fragment in &config.fragments {
        key = key.config(&format!("fragment.{}", fragment.name), &fragment.text);
    }
    for o in &config.overrides {
        key = key.config(&format!("override.{}", o.package), &o.input);
    }
    key = key.config("externals", config.externals.join("\n"));
    let path_of = |name: &str| -> PathBuf {
        dirs.iter()
            .find(|(n, _)| n == name)
            .map(|(_, dir)| dir.clone())
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
    let fragments = config
        .fragments
        .iter()
        .map(|f| {
            Ok(ConfigFragment {
                name: f.name.clone(),
                text: config::substitute(&f.text, &dirs)
                    .map_err(|why| format!("fragment {:?}: {why}", f.name))?,
            })
        })
        .collect::<Result<Vec<_>, String>>()
        .map_err(fail)?;
    let outputs: Vec<(&str, &Path)> = OUTPUTS
        .iter()
        .map(|(name, path)| (*name, Path::new(*path)))
        .collect();
    let made = Step::new(&store, cx, NAME, key.finish())
        .look_up_or_make(&outputs, |work| build(work, &src, &vars, &fragments, host))?;
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
fn in_store(store: &Store, tree: &Tree, what: &str) -> Result<(PathBuf, Digest), String> {
    let object = tree
        .object
        .object_ref()
        .ok_or_else(|| format!("{what} is not in the store"))?;
    Ok((store.path(&object), object.digest))
}

/// The body of the step: configure, resolve, make, then sort the outputs
/// out of Buildroot's output directory.
fn build(
    work: &mut Work<'_>,
    src: &Path,
    vars: &[String],
    fragments: &[ConfigFragment],
    host: &BuildrootHost,
) -> Result<(), StepError> {
    let fail = |why: String| StepError::body(NAME, why);
    let io_fail = |what: &'static str| {
        move |error: io::Error| StepError::body(NAME, format!("{what}: {error}"))
    };
    let dir = work.dir().to_path_buf();
    let out = dir.join("out");
    fs::create_dir(&out).map_err(io_fail("output directory"))?;

    work.run(&make_tool(host, src, &out, vars, &["allnoconfig"]))?;
    let config = out.join(".config");
    let base = fs::read_to_string(&config).map_err(io_fail("the base config"))?;
    // Last, so no description turns it off: the tar is where the root
    // filesystem's owners and modes are read from.
    let mut fragments = fragments.to_vec();
    fragments.push(ConfigFragment {
        name: "gaia.permissions".into(),
        text: perms::FRAGMENT.into(),
    });
    let merged = config::merge(&base, &fragments).map_err(fail)?;
    fs::write(&config, merged).map_err(io_fail("the merged config"))?;
    work.run(&make_tool(host, src, &out, vars, &["olddefconfig"]))?;
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
fn make_tool(
    host: &BuildrootHost,
    src: &Path,
    out: &Path,
    vars: &[String],
    goals: &[&str],
) -> Tool {
    let tool = if host.cpus.is_empty() {
        Tool::new(&host.make)
    } else {
        Tool::new(&host.taskset)
            .arg("-c")
            .arg(&host.cpus)
            .arg(&host.make)
    };
    tool.cwd(src)
        .arg("--no-print-directory")
        .arg(format!("-j{}", host.jobs))
        .arg(format!("O={}", out.display()))
        .arg(format!("BR2_DL_DIR={}", host.download_dir))
        .args(vars)
        .args(goals)
        .env("PATH", PATH)
        .env("LC_ALL", "C")
        .env("HOME", out.display().to_string())
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&OS_BUILDROOT_NODE];
