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
//! - **rootfs**: Buildroot's target tree without the files Buildroot keeps
//!   there for itself (`out::BOOKKEEPING`), with the owners and modes
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
//! Toolchain: unless a fragment sets or unsets a `BR2_TOOLCHAIN_BUILDROOT` or
//! `BR2_TOOLCHAIN_EXTERNAL` option, the build starts from Bootlin's prebuilt
//! glibc toolchain for the architecture its fragments name (the
//! `gaia.toolchain` defaults, see `config::defaults`), so Buildroot does not
//! compile GCC, glibc and the kernel headers first. A product that needs its
//! own toolchain sets one of those options, and then gets no default. The
//! defaults go in before the fragments, and a fragment's line for the same
//! symbol replaces the default's. Their lines are not checked the way the
//! fragments' are: where Bootlin does not serve a configuration, Kconfig falls
//! back and the build goes on with what Buildroot picks.
//!
//! The key holds the source's digest, each input tree's digest by name, the
//! fragments' text as written, the toolchain defaults when they apply, the
//! overrides, the externals and the make program. The download cache, the compiler cache's directory, the mirror,
//! the job count and the CPU list change how fast it runs, or where its
//! sources come from, not what it makes, so they are not in the key. The
//! compiler cache is on only when a description says `BR2_CCACHE=y`: that
//! line is in the key, and the cache's directory is not.
//!
//! Every make gets a fixed kernel build user, host and time (see `kbuild`), so
//! no image carries the builder's login or machine name.
//!
//! Settings, from the host (`gaia::project`), none of them in the key:
//!
//! - `GAIA_BUILDROOT_DL`: the download directory (`BR2_DL_DIR`). Default:
//!   `buildroot-downloads` beside the store's objects.
//! - `GAIA_BUILDROOT_DL_POOL`: the download pool, Buildroot's downloads by
//!   sha256 shared by every store of the user (see `dlpool`), so a new store
//!   or a key change does not fetch again. Default:
//!   `gaia/buildroot-download-pool` in `XDG_CACHE_HOME` or `~/.cache`;
//!   empty: none.
//! - `GAIA_BUILDROOT_MIRROR`: a primary download site (`BR2_PRIMARY_SITE`),
//!   a URI such as `https://mirror.example/buildroot`. Buildroot tries it
//!   first and then the package's own site and its backup site, so nothing
//!   is lost if the mirror lacks a file. Each download is checked against the
//!   hash Buildroot has for it, when there is one; a package with no hash
//!   file is not checked, so a mirror of such a package is trusted as much as
//!   the site it copies. Default: none.
//! - The compiler cache: `buildroot-ccache` beside the store's objects
//!   (`BR2_CCACHE_DIR`), shared by every build of the store, bounded to
//!   `CCACHE_MAX_SIZE`. Off by default (a build from nothing only pays for
//!   it); a description turns it on with `BR2_CCACHE=y`.
//!
//! Progress: Buildroot's `Installing to target` lines are counted as they
//! print, but the step kit reports progress only between tool runs, so the
//! count reaches Daedalus after `make` ends. See the PR for that gap.

mod config;
mod contract;
mod dev;
mod dlpool;
mod dtnames;
mod host;
mod jobs;
mod kbuild;
mod out;
mod patches;
mod perms;
mod place;
mod pool;
mod ppd;
mod progress;
mod times;
mod tools;

use daedalus::prelude::*;
use daedalus_cpu_author::FanIn;
use gaia_step::{Ctx, Digest, ErrorKind, KeyBuilder, Step, StepError, Store, Stream, Tool, Work};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub(crate) const NAME: &str = "gaia.os.buildroot";
/// 2: the root filesystem's owners and modes, from Buildroot's own tar.
/// (The check that every config line was taken changes no output, so no
/// new version.) 3: the host tools another step built, as an input. 4: the
/// kernel's build user, host and time are fixed (see `kbuild`). 5: a
/// kernel with Rust keeps `rust/` in `kernel-build` (see `out`). 6: every
/// make sees its output directory at one fixed path, and a pinned source date
/// (see `place`, `kbuild`). 7: the root filesystem is what Buildroot
/// packed under fakeroot, its post-fakeroot scripts' changes included, not
/// the target tree (see `perms`). 8: every make runs the deterministic `ar`
/// and `ranlib` and writes no bytecode on import (`tools`); the stored config
/// names the input trees, not their store paths; the planted seed names the
/// source by a marker.
const KEY_VERSION: u32 = 8;
/// The work directory an OS build expects when no earlier run recorded its
/// size: about what a full build leaves in its output directory.
const WORK_BYTES: u64 = 24 << 30;
/// The environment every make runs with. Nothing else is inherited.
pub(crate) const PATH: &str = "/usr/local/bin:/usr/bin:/bin";
/// The compiler cache's size bound, for every build of a store. It is an
/// environment variable of make, which ccache reads on each run, so a cache
/// made before it was set is kept to the bound too.
pub(crate) const CCACHE_MAX_SIZE: &str = "20G";

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
pub use host::{build_host, build_plan, host_goals, os_buildroot_host, os_buildroot_host_plan};

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
    /// Each input name with the path the run gives it: its store path, or
    /// for the plan an input it does not have, its placeholder (see
    /// [`placeholder`]). The names of the paths in the plan's text.
    pub paths: Vec<(String, PathBuf)>,
    /// `NAME=value` arguments for make: externals and source overrides.
    pub vars: Vec<String>,
    /// The fragments with each `@name@` replaced by its tree's path.
    pub fragments: Vec<ConfigFragment>,
    /// The toolchain defaults, applied before `fragments` and not checked
    /// against the resolved config. Empty when a fragment chooses a toolchain.
    pub defaults: Vec<ConfigFragment>,
    /// The contracts the kernel's final config must meet, each with the
    /// path of its file in the store.
    pub contracts: Vec<(gaia_types::KernelContract, PathBuf)>,
    /// The value of the key entry the Gaia patches add, when the choice is
    /// not the default (see [`patches`]). `None` for the default.
    pub patch_key: Option<String>,
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
            .config("make", &host.make)
            .config(
                "seen",
                place::seen(host, Path::new("")).display().to_string(),
            );
        for (name, _, digest) in &self.dirs {
            key = key.input(&format!("input.{name}"), *digest);
        }
        // Only when there are some: a build that chooses its toolchain keeps
        // the key it had.
        for fragment in &self.defaults {
            key = key.config(&format!("defaults.{}", fragment.name), &fragment.text);
        }
        key = self.patch_config(key);
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

    /// `key` with the entry the Gaia patches add, when there is one. The
    /// source's digest in the key is the unpatched one.
    /// The parts of a dev build's identity (see `dev`): the key without the
    /// config text and the input trees' content.
    pub fn identity(
        &self,
        name: &str,
        version: u32,
        config: &OsConfig,
        host: &BuildrootHost,
        seed: Digest,
    ) -> Vec<(String, String)> {
        let mut parts = vec![
            ("step".to_string(), format!("{name} {version}")),
            ("source".to_string(), self.src_digest.hex()),
            (
                "patches".to_string(),
                self.patch_key.clone().unwrap_or_default(),
            ),
            ("seed".to_string(), seed.hex()),
            ("make".to_string(), host.make.clone()),
        ];
        for fragment in &self.defaults {
            parts.push((format!("defaults.{}", fragment.name), fragment.text.clone()));
        }
        let names = |list: Vec<&str>| list.join(" ");
        parts.push((
            "fragments".into(),
            names(config.fragments.iter().map(|f| f.name.as_str()).collect()),
        ));
        parts.push((
            "inputs".into(),
            names(self.dirs.iter().map(|(n, _, _)| n.as_str()).collect()),
        ));
        for o in &config.overrides {
            parts.push((format!("override.{}", o.package), o.input.clone()));
        }
        parts.push(("externals".into(), config.externals.join("\n")));
        parts
    }

    pub fn patch_config(&self, key: KeyBuilder) -> KeyBuilder {
        match &self.patch_key {
            Some(value) => key.config(patches::KEY_NAME, value),
            None => key,
        }
    }
}

/// The root of the placeholder paths the plan gives the input trees it does
/// not have. Nothing is there: the plan reads none of those trees (only
/// what `OsConfig::plan_inputs` names), and one path for every run keeps the
/// plan's text and key the same wherever the trees are stored.
pub(crate) const PLACEHOLDER: &str = "/gaia-plan-placeholder";

/// The placeholder path of the input `name` in the plan.
pub(crate) fn placeholder(name: &str) -> PathBuf {
    Path::new(PLACEHOLDER).join(name)
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
    let paths = dirs
        .iter()
        .map(|(name, dir, _)| (name.clone(), dir.clone()))
        .collect();
    assemble(src, src_digest, dirs, paths, config)
}

/// What the host plan is given: the Buildroot source and the config, and the
/// input trees the plan reads or needs (`OsConfig::plan_inputs`, in that
/// order). Every other input is at its placeholder path, which is all the
/// plan knows of it: its text and key do not depend on the trees.
pub(crate) fn prepare_plan(
    store: &Store,
    source: &Tree,
    config: &OsConfig,
    trees: &[Tree],
    host: &BuildrootHost,
) -> Result<Prepared, String> {
    prepare_given(store, source, config, &config.plan_inputs(), trees, host)
}

/// As [`prepare_plan`], with the input trees named `needed` (in that order)
/// given and every other input at its placeholder path: the host tools'
/// (`OsConfig::host_inputs`).
pub(crate) fn prepare_given(
    store: &Store,
    source: &Tree,
    config: &OsConfig,
    needed: &[String],
    trees: &[Tree],
    host: &BuildrootHost,
) -> Result<Prepared, String> {
    check(config, config.inputs.len(), host)?;
    if trees.len() != needed.len() {
        return Err(format!(
            "{} input trees are needed ({}), and {} are given",
            needed.len(),
            needed.join(", "),
            trees.len()
        ));
    }
    let (src, src_digest) = in_store(store, source, "the Buildroot source")?;
    let mut dirs = Vec::with_capacity(trees.len());
    for (name, tree) in needed.iter().zip(trees) {
        let (dir, digest) = in_store(store, tree, &format!("input {name}"))?;
        dirs.push((name.clone(), dir, digest));
    }
    let paths = config
        .inputs
        .iter()
        .map(|name| {
            let path = dirs
                .iter()
                .find(|(n, _, _)| n == name)
                .map(|(_, dir, _)| dir.clone())
                .unwrap_or_else(|| placeholder(name));
            (name.clone(), path)
        })
        .collect();
    assemble(src, src_digest, dirs, paths, config)
}

/// The `Prepared` of a run: its make variables, its fragments with each
/// `@name@` replaced by the input's path, and its contracts.
fn assemble(
    src: PathBuf,
    src_digest: Digest,
    dirs: Vec<(String, PathBuf, Digest)>,
    paths: Vec<(String, PathBuf)>,
    config: &OsConfig,
) -> Result<Prepared, String> {
    let path_of = |name: &str| -> PathBuf {
        paths
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, path)| path.clone())
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
    // The device trees asked of the kernel build are in its tree, when the
    // step is given that tree (the host plan has only its placeholder).
    if let Some(kernel) = config.overrides.iter().find(|o| o.package == "LINUX")
        && let Some((_, tree, _)) = dirs.iter().find(|(name, _, _)| *name == kernel.input)
        && let Some(why) = dtnames::missing(&fragments, tree)
    {
        return Err(why);
    }
    let defaults = config::defaults(&fragments);
    Ok(Prepared {
        src,
        src_digest,
        dirs,
        paths,
        vars,
        fragments,
        defaults,
        contracts,
        patch_key: None,
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
    let mut prepared = prepare(&store, &source, config, inputs, host).map_err(fail)?;
    let mode = patches::mode_from_env().map_err(fail)?;
    patches::apply_to(&store, cx, &mut prepared, mode)?;
    let (seed_dir, seed_digest) = in_store(&store, seed, "the host tools").map_err(fail)?;
    let key = step_key(&prepared, config, host, seed_digest);
    let outputs: Vec<(&str, &Path)> = OUTPUTS
        .iter()
        .map(|(name, path)| (*name, Path::new(*path)))
        .collect();
    let warm = if host.dev_reuse.is_empty() {
        None
    } else {
        let mut parts = prepared.identity(NAME, KEY_VERSION, config, host, seed_digest);
        parts.push(("dev".to_string(), dev::VERSION.to_string()));
        let identity = dev::identity(&parts);
        Some((PathBuf::from(&host.dev_reuse), identity))
    };
    let step = Step::new(&store, cx, NAME, key.finish());
    // The seed's plant, known before the work directory is chosen: the
    // host twice (LLVM's is 2 GB), whatever the last build of another seed
    // used (see `ppd::plant_bytes`).
    let plant = ppd::plant_bytes(&seed_dir)
        .map_err(|e| StepError::body(NAME, format!("the host tools' size: {e}")))?;
    let made = match &warm {
        None => step
            .ram_work(WORK_BYTES)
            .ram_floor(plant)
            .look_up_or_make(&outputs, |work| {
                build(work, &prepared, &seed_dir, host, None)
            })?,
        // On the store's own disk: the warm directory is there, and the
        // outputs are copied out of it.
        Some((root, identity)) => step.look_up_or_make(&outputs, |work| {
            let warm = dev::Warm::open(root, identity)
                .map_err(|e| StepError::body(NAME, format!("the dev build's directory: {e}")))?;
            build(work, &prepared, &seed_dir, host, Some(&warm))
        })?,
    };
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

/// The step's key. A dev build's is never a clean build's (see `dev`).
fn step_key(
    prepared: &Prepared,
    config: &OsConfig,
    host: &BuildrootHost,
    seed: Digest,
) -> KeyBuilder {
    let key = prepared
        .key(NAME, KEY_VERSION, config, host)
        .input("seed", seed);
    if host.dev_reuse.is_empty() {
        key
    } else {
        key.config("build", format!("dev-reuse {}", dev::VERSION))
    }
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
    if !host.ccache_dir.is_empty() && !Path::new(&host.ccache_dir).is_absolute() {
        return Err(format!(
            "ccache directory {:?} is not an absolute path",
            host.ccache_dir
        ));
    }
    if !host.mirror.is_empty() {
        check_mirror(&host.mirror)?;
    }
    Ok(())
}

/// A mirror is a URI Buildroot's download helper reads: a scheme, then a
/// path, with no white space or quotes, since it is given to make as a
/// command-line word and written into the download commands.
pub fn check_mirror(url: &str) -> Result<(), String> {
    let plain = !url.contains(|c: char| c.is_whitespace() || "\\\"'$".contains(c));
    if url.contains("://") && plain {
        Ok(())
    } else {
        Err(format!(
            "mirror {url:?} is not a URI (scheme://path, with no white space or quotes)"
        ))
    }
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

/// Whether a resolved config makes Buildroot build every host tool from
/// source. Buildroot's own `check-host-*` rules then never use the host's
/// bison, flex, xz, cmake or Python, so Gaia never sets it.
pub(crate) fn forces_host_build(resolved: &str) -> bool {
    resolved
        .lines()
        .any(|line| line.trim() == "BR2_FORCE_HOST_BUILD=y")
}

/// The fragments a config is merged from, and the ones whose lines must all
/// have been taken: `(applied, checked)`. Applied: the toolchain defaults,
/// then the descriptions, then the permissions, last so no description turns
/// them off. Checked: the descriptions and the permissions, not the defaults.
pub(crate) fn merged_fragments(prepared: &Prepared) -> (Vec<ConfigFragment>, Vec<ConfigFragment>) {
    let mut checked = prepared.fragments.clone();
    checked.push(ConfigFragment {
        name: "gaia.permissions".into(),
        text: perms::FRAGMENT.into(),
    });
    let mut applied = prepared.defaults.clone();
    applied.extend(checked.iter().cloned());
    (applied, checked)
}

/// The body of the step: configure, resolve, make, then sort the outputs
/// out of Buildroot's output directory.
/// Buildroot's config in `out`: its `allnoconfig`, the toolchain defaults and
/// the fragments over it, resolved, with every fragment line checked to have
/// been taken. Returns the resolved text.
pub(crate) fn configure(
    step: &'static str,
    work: &mut Work<'_>,
    prepared: &Prepared,
    out: &Path,
    host: &BuildrootHost,
    slot: &jobs::Slot<'_>,
) -> Result<String, StepError> {
    let fail = |why: String| StepError::body(step, why);
    let io_fail = |what: &'static str| {
        move |error: io::Error| StepError::body(step, format!("{what}: {error}"))
    };
    let (src, vars) = (&prepared.src, &prepared.vars);
    tools::write(out).map_err(io_fail("the deterministic archivers"))?;
    dlpool::before(step, work, src, host)?;
    work.run(&make_tool(host, src, out, vars, &["allnoconfig"], slot))?;
    let config = out.join(".config");
    let base = fs::read_to_string(&config).map_err(io_fail("the base config"))?;
    let (applied, fragments) = merged_fragments(prepared);
    let merged = config::merge(&base, &applied).map_err(fail)?;
    fs::write(&config, merged).map_err(io_fail("the merged config"))?;
    work.run(&make_tool(host, src, out, vars, &["olddefconfig"], slot))?;
    // What Buildroot resolved must be what was asked for.
    let resolved = fs::read_to_string(&config).map_err(io_fail("the resolved config"))?;
    if forces_host_build(&resolved) {
        return Err(fail(
            "BR2_FORCE_HOST_BUILD=y: Buildroot would build every host tool from source \
             and ignore the host's; the configuration must not set it"
                .into(),
        ));
    }
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

/// The OS build's make, and then its packages' times in the record, whether
/// the make failed or not.
fn build(
    work: &mut Work<'_>,
    prepared: &Prepared,
    seed: &Path,
    host: &BuildrootHost,
    warm: Option<&dev::Warm>,
) -> Result<(), StepError> {
    let out = match warm {
        Some(warm) => warm.out(),
        None => work.dir().join("out"),
    };
    // How full the shared pool was while this build ran.
    let sampler = crate::jobs::sample(&host.make, usize::try_from(host.jobs).unwrap_or(usize::MAX));
    let label = part_label(prepared);
    let result = in_slot(NAME, work, host, label, |work, slot| {
        build_steps(work, prepared, seed, host, warm, slot)
    });
    if let Some(sampler) = sampler {
        work.detail(crate::jobs::SAMPLE_DETAIL, sampler.finish());
    }
    times::record(work, &out, host.jobs);
    result
}

fn build_steps(
    work: &mut Work<'_>,
    prepared: &Prepared,
    seed: &Path,
    host: &BuildrootHost,
    warm: Option<&dev::Warm>,
    slot: &jobs::Slot<'_>,
) -> Result<(), StepError> {
    let fail = |why: String| StepError::body(NAME, why);
    let io_fail = |what: &'static str| {
        move |error: io::Error| StepError::body(NAME, format!("{what}: {error}"))
    };
    let (src, vars) = (&prepared.src, &prepared.vars);
    let dir = work.dir().to_path_buf();
    let (out, resolved, manifest) = match warm {
        None => {
            let out = dir.join("out");
            let resolved = fresh_out(work, prepared, seed, host, slot, &out)?;
            (out, resolved, None)
        }
        Some(warm) => {
            let (resolved, manifest) = warm_out(work, prepared, seed, host, slot, warm)?;
            (warm.out(), resolved, Some(manifest))
        }
    };
    let config = out.join(".config");

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
        work.run(&make_tool(host, src, &out, vars, &[goal.as_str()], slot))?;
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
    let order = make_tool(host, src, &out, vars, &["show-build-order"], slot).on_line(
        move |stream, line| {
            if stream == Stream::Stdout
                && let Ok(mut text) = sink.lock()
            {
                text.push_str(line);
                text.push('\n');
            }
        },
    );
    work.run(&order)?;
    let listed = shown.lock().map(|text| text.clone()).unwrap_or_default();
    let targets = progress::target_packages(&listed);
    let total = u32::try_from(targets.len()).unwrap_or(u32::MAX);
    work.progress(0, total);

    let installed = Arc::new(Mutex::new(BTreeSet::new()));
    let sink = installed.clone();
    let jobs = pool::Shared::default();
    let jobs_sink = jobs.clone();
    let mismatches = dlpool::Mismatches::default();
    let seen = mismatches.clone();
    let build = make_tool(host, src, &out, vars, &[], slot).on_line(move |_, line| {
        pool::feed(&jobs_sink, line);
        seen.line(line);
        if let Some(name) = progress::installed_to_target(line)
            && let Ok(mut set) = sink.lock()
        {
            set.insert(name.to_string());
        }
    });
    let made = work.run(&build);
    pool::record(work, &jobs);
    let made = dlpool::after(NAME, work, src, host, &mismatches, made);
    // The planted host tools are links every seeded package shares: one
    // written in place through any of them fails the build (see `ppd`).
    // Checked when make failed too: the write may be why it failed, and a
    // failure named for something else must not hide it.
    let started = std::time::Instant::now();
    let changed = ppd::plant_changes(&out).map_err(io_fail("the planted host tools"))?;
    work.detail(
        "plant-check",
        serde_json::json!({ "changed": changed.len(), "seconds": started.elapsed().as_secs_f64() }),
    );
    if !changed.is_empty() {
        let make = match &made {
            Ok(_) => String::new(),
            Err(error) => format!("\nmake failed as well: {error}"),
        };
        return Err(fail(format!(
            "{} planted host tool file{} changed in place during the build; the seed's \
             packages share them as hard links, so the change reached every one of them:\n  {}{make}",
            changed.len(),
            if changed.len() == 1 { " was" } else { "s were" },
            changed.join("\n  ")
        )));
    }
    made?;
    let installed = installed.lock().map(|set| set.clone()).unwrap_or_default();
    work.progress(progress::done(&installed, &targets), total);

    // The root filesystem is what Buildroot packed under fakeroot, after its
    // device and permission tables, mkusers, the post-fakeroot scripts and
    // the presets: the tar's files, and its owners and modes as the tree's
    // entries (see `perms`). The tar itself is not an output. Without a tar
    // (a build that packs no root filesystem) it is the target tree.
    let tar = out.join("images").join(perms::TAR);
    let rootfs = dir.join("rootfs");
    let entries = if tar.is_file() {
        let started = std::time::Instant::now();
        let unpacked =
            perms::unpack(&tar, &rootfs).map_err(io_fail("the root filesystem's tar"))?;
        let entries = perms::entries(&tar, &rootfs)
            .map_err(io_fail("the root filesystem's owners and modes"))?;
        fs::remove_file(&tar).map_err(io_fail("the root filesystem's tar"))?;
        work.detail(
            "rootfs",
            serde_json::json!({
                "from": perms::TAR,
                "devices": unpacked.devices,
                "devices_left_out": perms::DEVICES_LEFT_OUT,
                "seconds": started.elapsed().as_secs_f64(),
            }),
        );
        let outside = perms::outside_dev(&unpacked.devices);
        if !outside.is_empty() {
            return Err(fail(format!(
                "the root filesystem has {} device node{} outside /dev, which the tree cannot \
                 hold (only /dev's are left out, since devtmpfs covers /dev at boot):\n  {}",
                outside.len(),
                if outside.len() == 1 { "" } else { "s" },
                outside.join("\n  ")
            )));
        }
        entries
    } else {
        take(&out.join("target"), &rootfs, warm.is_some()).map_err(io_fail("the target tree"))?;
        Vec::new()
    };
    let text = serde_json::to_vec(&entries).map_err(|e| fail(e.to_string()))?;
    fs::write(dir.join("rootfs-entries.json"), text).map_err(io_fail("the entries"))?;
    // What Buildroot leaves at the top of its target tree for its own use:
    // its image steps leave these out, and so does this.
    for name in out::BOOKKEEPING {
        match fs::remove_file(rootfs.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_fail("Buildroot's own file in the target")(error)),
        }
    }
    let left = out::bookkeeping_in(&dir.join("rootfs"));
    if !left.is_empty() {
        return Err(fail(format!(
            "the root filesystem holds Buildroot's own files, which are no part of an image: {}",
            left.join(", ")
        )));
    }
    out::copy_tree(&dir.join("rootfs/lib/modules"), &dir.join("modules"))
        .map_err(io_fail("the kernel modules"))?;
    out::sort_images(&out.join("images"), &dir).map_err(io_fail("the images"))?;
    // The bootloader's final configuration, beside what it made: what a
    // claim about a built U-Boot is read from, as the kernel's is from its
    // build tree.
    if let Some(final_config) =
        out::package_config(&out.join("build"), "uboot").map_err(io_fail("U-Boot's config"))?
    {
        gaia_store::copy_file(&final_config, dir.join("bootloader/u-boot.config"))
            .map_err(io_fail("U-Boot's config"))?;
    }
    fs::create_dir(dir.join("config")).map_err(io_fail("the config directory"))?;
    // The input trees, the source and the directories of this run by their
    // names, as in the host plan: the same config from any store.
    let text = fs::read_to_string(&config).map_err(io_fail("the config"))?;
    let names = host::run_names(prepared, host, &out);
    fs::write(
        dir.join("config/buildroot.config"),
        host::named(&text, &names),
    )
    .map_err(io_fail("the config"))?;
    match out::kernel_build_source(&out.join("build")).map_err(io_fail("the kernel build"))? {
        Some(source) => {
            out::copy_kernel_build(&source, &dir.join("kernel-build"))
                .map_err(io_fail("the kernel build tree"))?;
            // Buildroot's host directory holds the cross compiler it built or
            // installed, with its sysroot. Its text files and links name the
            // output path (a `.pc` prefix, a script's interpreter): the
            // marker stands for it, so the stored tree is the same whether
            // the build ran in RAM or on disk.
            take(&out.join("host"), &dir.join("toolchain"), warm.is_some())
                .map_err(io_fail("the toolchain"))?;
            host::mark_out(
                &dir.join("toolchain"),
                &place::seen(host, &out).display().to_string(),
            )
            .map_err(io_fail("the toolchain's paths"))?;
            // Archives an archiver wrote with real dates (a cross ar, which
            // the wrappers do not reach), as `D` writes them.
            let rewritten = tools::deterministic_archives(&dir.join("toolchain"))
                .map_err(io_fail("the toolchain's archives"))?;
            work.detail(
                "archives-rewritten",
                serde_json::json!({ "toolchain": rewritten.len(), "first": rewritten.iter().take(5).collect::<Vec<_>>() }),
            );
        }
        None => {
            fs::create_dir(dir.join("kernel-build")).map_err(io_fail("the kernel build"))?;
            fs::create_dir(dir.join("toolchain")).map_err(io_fail("the toolchain"))?;
        }
    }
    if let (Some(warm), Some(manifest)) = (warm, manifest) {
        warm.finish(&manifest)
            .map_err(io_fail("the dev build's manifest"))?;
    }
    Ok(())
}

/// Moves a tree out of Buildroot's output directory; copies it when the
/// directory is kept (a dev build).
fn take(from: &Path, to: &Path, keep: bool) -> io::Result<()> {
    if keep {
        out::copy_tree(from, to)
    } else {
        fs::rename(from, to)
    }
}

/// A new output directory `out`, with the seed planted and the config
/// resolved. Returns the resolved config.
fn fresh_out(
    work: &mut Work<'_>,
    prepared: &Prepared,
    seed: &Path,
    host: &BuildrootHost,
    slot: &jobs::Slot<'_>,
    out: &Path,
) -> Result<String, StepError> {
    let io_fail = |what: &'static str| {
        move |error: io::Error| StepError::body(NAME, format!("{what}: {error}"))
    };
    fs::create_dir_all(out).map_err(io_fail("output directory"))?;
    // The host tools another step built, before Buildroot looks: it finds
    // them installed and builds none of them again.
    host::plant(seed, out, &place::seen(host, out), &prepared.src)
        .map_err(io_fail("the host tools"))?;
    let resolved = configure(NAME, work, prepared, out, host, slot)?;
    // With per-package directories on, the seed's packages get their own
    // trees, complete and installed, before anything builds.
    let started = std::time::Instant::now();
    let files = host::plant_per_package(seed, out, &resolved)
        .map_err(io_fail("the host tools' per-package directories"))?;
    if files > 0 {
        work.detail(
            "plant",
            serde_json::json!({ "files": files, "seconds": started.elapsed().as_secs_f64() }),
        );
    }
    Ok(resolved)
}

/// The warm directory made ready for the make (see `dev`): kept, with what
/// changed removed, or emptied and made fresh. Returns the resolved config
/// and the manifest to record when the build succeeds. The decision is in
/// the step's record.
fn warm_out(
    work: &mut Work<'_>,
    prepared: &Prepared,
    seed: &Path,
    host: &BuildrootHost,
    slot: &jobs::Slot<'_>,
    warm: &dev::Warm,
) -> Result<(String, dev::Manifest), StepError> {
    let fail = |why: String| StepError::body(NAME, why);
    let io_fail = |what: &'static str| {
        move |error: io::Error| StepError::body(NAME, format!("{what}: {error}"))
    };
    let (src, vars) = (&prepared.src, &prepared.vars);
    let out = warm.out();
    let old = warm.manifest().filter(|_| out.is_dir());
    warm.begin().map_err(io_fail("the dev build's manifest"))?;
    let mut places: Vec<(PathBuf, String)> = prepared
        .dirs
        .iter()
        .map(|(name, path, _)| (path.clone(), format!("@{name}@")))
        .collect();
    places.push((prepared.src.clone(), "@buildroot@".into()));
    places.push((out.clone(), "@out@".into()));
    places.push((place::seen(host, &out), "@out@".into()));
    let kconfig_dirclean = std::env::var(dev::KCONFIG_ENV).is_ok_and(|v| v == "dirclean");
    let reuse = if old.is_some() {
        let resolved = configure(NAME, work, prepared, &out, host, slot)?;
        let info = host::captured(work, make_tool(host, src, &out, vars, &["show-info"], slot))?;
        let new = dev::fingerprint(&resolved, &info, &places).map_err(fail)?;
        let reverse = dev::reverse_dependencies(&info).map_err(fail)?;
        let reuse = if ppd::enabled(&resolved) {
            dev::plan(old.as_ref(), &new, &reverse, kconfig_dirclean)
        } else {
            dev::Reuse::Clean("per-package directories are off".into())
        };
        if let dev::Reuse::Keep {
            dirclean,
            reconfigure,
        } = &reuse
        {
            if !dirclean.is_empty() {
                let goals: Vec<String> = dirclean.iter().map(|p| format!("{p}-dirclean")).collect();
                let goals: Vec<&str> = goals.iter().map(String::as_str).collect();
                work.run(&make_tool(host, src, &out, vars, &goals, slot))?;
            }
            for package in reconfigure {
                let build_dir = dev::build_dir(&info, package).ok_or_else(|| {
                    fail(format!(
                        "make show-info names no build directory for {package}"
                    ))
                })?;
                dev::reconfigure(&out, package, &build_dir)
                    .map_err(io_fail("a package configured again"))?;
            }
            // Made again: target/ from every package's own, by each make;
            // images/ by each package installing its images again.
            dev::forget_images(&out).map_err(io_fail("the packages' images installed again"))?;
            for made in ["target", "images"] {
                match fs::remove_dir_all(out.join(made)) {
                    Err(error) if error.kind() != io::ErrorKind::NotFound => {
                        return Err(io_fail("the last dev build's target and images")(error));
                    }
                    _ => {}
                }
            }
            work.detail(
                "dev-reuse",
                serde_json::json!({ "kept": true, "dirclean": dirclean, "reconfigure": reconfigure }),
            );
            return Ok((resolved, new));
        }
        reuse
    } else {
        dev::Reuse::Clean("no earlier dev build of this identity".into())
    };
    let dev::Reuse::Clean(why) = reuse else {
        unreachable!("a kept directory returned above")
    };
    warm.clear().map_err(io_fail("the dev build's directory"))?;
    let resolved = fresh_out(work, prepared, seed, host, slot, &out)?;
    let info = host::captured(work, make_tool(host, src, &out, vars, &["show-info"], slot))?;
    let new = dev::fingerprint(&resolved, &info, &places).map_err(fail)?;
    work.detail(
        "dev-reuse",
        serde_json::json!({ "kept": false, "why": why }),
    );
    Ok((resolved, new))
}

/// This build's slot in the process's make jobserver (see [`jobs`]), taken
/// now and held until the build is done.
pub(crate) fn admit(
    step: &'static str,
    host: &BuildrootHost,
    cancelled: &dyn Fn() -> bool,
    yields: bool,
    label: &str,
) -> Result<jobs::Slot<'static>, StepError> {
    let jobs = usize::try_from(host.jobs).unwrap_or(usize::MAX);
    jobs::admit(&host.make, jobs, cancelled, yields, label).map_err(|refused| match refused {
        jobs::Refused::Cancelled => StepError::new(step, ErrorKind::Cancelled, Vec::new()),
        jobs::Refused::Pool(why) => StepError::body(step, why),
    })
}

/// Runs `body` in this build's slot in the process's jobserver, held until
/// the build's last make is done. The slot yields to the builds without
/// slack when the step's records say this one has some (see
/// `gaia_step::slack`), and the step records what that did, or why it did
/// not yield, as [`jobs::YIELD_DETAIL`]. `label` names the build in the
/// other builds' records.
pub(crate) fn in_slot<T>(
    step: &'static str,
    work: &mut Work<'_>,
    host: &BuildrootHost,
    label: &str,
    body: impl FnOnce(&mut Work<'_>, &jobs::Slot<'_>) -> Result<T, StepError>,
) -> Result<T, StepError> {
    // Runs are compared at the same job count, so the record has it first.
    work.jobs(host.jobs);
    let decision = work.slack();
    let slot = admit(step, host, &|| work.cancelled(), decision.yields(), label)?;
    let result = body(work, &slot);
    let detail = match slot.yielded() {
        Some(done) => {
            work.yielded();
            done.detail(&decision.to_string())
        }
        None if decision.yields() => serde_json::json!({
            "yielded": false,
            "why": format!(
                "shares the main pool: {} ({decision})",
                slot.shares().unwrap_or("no yielding pool")
            ),
        }),
        None => serde_json::json!({ "yielded": false, "why": decision.to_string() }),
    };
    let mut detail = detail;
    detail["label"] = label.into();
    work.detail(jobs::YIELD_DETAIL, detail);
    result
}

/// Which of an image's Buildroot builds `prepared` is, for the records: the
/// kernel's and the bootloader's have the board's fragment of that name
/// (see the graph's OS split), and the root filesystem's has neither.
fn part_label(prepared: &Prepared) -> &'static str {
    let has = |name: &str| prepared.fragments.iter().any(|f| f.name == name);
    if has("board.kernel") {
        "kernel"
    } else if has("board.bootloader") {
        "bootloader"
    } else {
        "root filesystem"
    }
}

/// The make command for `goals`: its arguments, in order, for a test to
/// read. The job count is on the command line.
pub fn make_arguments(
    host: &BuildrootHost,
    out: &Path,
    vars: &[String],
    goals: &[&str],
) -> Vec<String> {
    arguments(host, out, vars, goals, false)
}

/// The arguments of [`make_arguments`]; with `shared`, no job count, since
/// the jobserver's `MAKEFLAGS` carries it.
fn arguments(
    host: &BuildrootHost,
    out: &Path,
    vars: &[String],
    goals: &[&str],
    shared: bool,
) -> Vec<String> {
    let mut args = Vec::new();
    if !host.cpus.is_empty() {
        args.extend([host.taskset.clone(), "-c".into(), host.cpus.clone()]);
    }
    args.push(host.make.clone());
    args.push("--no-print-directory".into());
    if !shared {
        args.push(format!("-j{}", host.jobs));
    }
    // What Buildroot gives the builds that take a job count of their own
    // (`cmake --build -j`, Go's `-p`, waf's `-j`) instead of the jobserver.
    // Left to Buildroot it is the machine's processor count plus one, not
    // the CPUs this build may use, and such a build left the pool with it
    // (host-ninja ran `-j33` beside a pool of 26).
    args.push(format!("PARALLEL_JOBS={}", host.jobs));
    args.push(format!("O={}", out.display()));
    args.push(format!("BR2_DL_DIR={}", host.download_dir));
    // Command-line variables, not config lines: a path is then in no
    // `.config` text or key. The mirror is not BR2_PRIMARY_SITE_ONLY=n:
    // Buildroot tests that for emptiness, so `n` would drop the upstream.
    if !host.ccache_dir.is_empty() {
        args.push(format!("BR2_CCACHE_DIR={}", host.ccache_dir));
    }
    if !host.mirror.is_empty() {
        args.push(format!("BR2_PRIMARY_SITE={}", host.mirror));
    }
    args.extend(vars.iter().cloned());
    args.extend(goals.iter().map(|goal| (*goal).to_string()));
    args
}

/// One make run in `out`, for `goals` (none means Buildroot's default,
/// `all`), in `slot`. With the pool's fifo the make gets the jobserver in
/// `MAKEFLAGS` and no job count: Buildroot's own make then adds no `-j` to
/// its package makes, which take their jobs from the same pool. Without a
/// fifo the job count is on the command line, and the slot is a turn. With a
/// CPU list, make runs under `taskset`.
pub(crate) fn make_tool(
    host: &BuildrootHost,
    src: &Path,
    out: &Path,
    vars: &[String],
    goals: &[&str],
    slot: &jobs::Slot<'_>,
) -> Tool {
    let seen = place::seen(host, out);
    let words = arguments(host, &seen, vars, goals, slot.makeflags().is_some());
    let mut words = place::wrapped(host, out, words).into_iter();
    let program = words.next().unwrap_or_default();
    let mut tool = Tool::new(program)
        .cwd(src)
        .args(words)
        .env("PATH", tools::path(&seen))
        .env("LC_ALL", "C")
        // A Python module imported during the build is compiled in memory,
        // not written beside its source with that source's time in it (a
        // host package's site-packages, imported by the next package's
        // build). Bytecode a package compiles on purpose is not affected.
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("CCACHE_MAXSIZE", CCACHE_MAX_SIZE)
        .env("HOME", seen.display().to_string())
        .refuse_repeated(REPEATS, REPEATS_WHY);
    for (name, value) in kbuild::env(src) {
        tool = tool.env(name, value);
    }
    // A yielding build runs nicer than Gaia (see `jobs::NICE_STEP`).
    let tool = match slot.nice() {
        Some(nice) => tool.nice(nice),
        None => tool,
    };
    match slot.makeflags() {
        Some(flags) => {
            let mut tool = tool.env("MAKEFLAGS", flags);
            for line in JOBSERVER_LOST {
                tool = tool.refuse_line(line, JOBSERVER_WHY);
            }
            tool
        }
        None => tool,
    }
}

/// How many times in a row a make may print one line. Kconfig, asked for a
/// symbol that has no value and no default (a hex or string one) with no
/// terminal to read, prints the prompt and "Error in reading or end of
/// file." again for ever: one rock-5c U-Boot build wrote 11.7 GB of it in
/// 85 minutes and never failed. A build's own output does not repeat one
/// line this often.
pub(crate) const REPEATS: usize = 1000;
const REPEATS_WHY: &str = "make printed the same line over and over: a Kconfig symbol with no value and no default asks for one, and no terminal answers (give it a value in the config, or turn off what needs it)";

/// What GNU make prints when it cannot use the jobserver its `MAKEFLAGS`
/// names: a warning, and it goes on with one job. A pool make that prints
/// either fails at once, so a build that would run serially never passes.
pub(crate) const JOBSERVER_LOST: [&str; 2] = ["cannot open jobserver", "jobserver unavailable"];
const JOBSERVER_WHY: &str =
    "make could not use the build's job pool and would run one job at a time";

/// The node, for a registry.
pub const NODES: [&NodeRecord; 3] = [
    &OS_BUILDROOT_NODE,
    &host::OS_BUILDROOT_HOST_PLAN_NODE,
    &host::OS_BUILDROOT_HOST_NODE,
];

/// A `Prepared` for the key and merge tests: the source and no input trees,
/// with these fragments and toolchain defaults.
#[cfg(test)]
fn prepared_for_test(fragments: Vec<ConfigFragment>, defaults: Vec<ConfigFragment>) -> Prepared {
    Prepared {
        src: PathBuf::from("/src"),
        src_digest: Digest::of_bytes(b"source"),
        dirs: Vec::new(),
        paths: Vec::new(),
        vars: Vec::new(),
        fragments,
        defaults,
        contracts: Vec::new(),
        patch_key: None,
    }
}

#[cfg(test)]
mod toolchain_tests {
    use super::*;

    fn fragment(name: &str, text: &str) -> ConfigFragment {
        ConfigFragment {
            name: name.into(),
            text: text.into(),
        }
    }

    /// A dev build's key is never a clean build's, and where it is kept
    /// does not change it.
    #[test]
    fn a_dev_key_is_never_a_clean_key() {
        let prepared = prepared_for_test(Vec::new(), Vec::new());
        let seed = Digest::of_bytes(b"seed");
        let config = OsConfig::default();
        let mut host = host_for_test();
        let clean = step_key(&prepared, &config, &host, seed).finish();
        host.dev_reuse = "/store/dev-buildroot".into();
        let dev = step_key(&prepared, &config, &host, seed).finish();
        host.dev_reuse = "/elsewhere/dev-buildroot".into();
        let moved = step_key(&prepared, &config, &host, seed).finish();
        assert_ne!(clean, dev);
        assert_eq!(dev, moved);
    }

    /// A warm directory's identity names the fragments, not their text: a
    /// config change keeps the directory, and another set of fragments (the
    /// kernel's part against the root's) has its own.
    #[test]
    fn the_identity_names_fragments_not_their_text() {
        let fragment = |name: &str, text: &str| ConfigFragment {
            name: name.into(),
            text: text.into(),
        };
        let identity = |fragments: Vec<ConfigFragment>| {
            let config = OsConfig {
                fragments,
                ..OsConfig::default()
            };
            let prepared = prepared_for_test(Vec::new(), Vec::new());
            dev::identity(&prepared.identity(
                NAME,
                KEY_VERSION,
                &config,
                &host_for_test(),
                Digest::of_bytes(b"seed"),
            ))
        };
        let base = identity(vec![fragment("os", "BR2_PACKAGE_A=y\n")]);
        assert_eq!(
            base,
            identity(vec![fragment("os", "BR2_PACKAGE_A=y\nBR2_PACKAGE_B=y\n")])
        );
        assert_ne!(
            base,
            identity(vec![
                fragment("gaia.no-root", ""),
                fragment("os", "BR2_PACKAGE_A=y\n")
            ])
        );
    }

    fn key_of(prepared: &Prepared) -> gaia_step::StepKey {
        prepared
            .key(
                "gaia.os.buildroot",
                KEY_VERSION,
                &OsConfig::default(),
                &host_for_test(),
            )
            .finish()
    }

    fn host_for_test() -> BuildrootHost {
        BuildrootHost {
            make: "/usr/bin/make".into(),
            taskset: "/usr/bin/taskset".into(),
            cpus: String::new(),
            jobs: 4,
            download_dir: "/dl".into(),
            download_pool: String::new(),
            config_only: false,
            dev_reuse: String::new(),
            bwrap: String::new(),
            ccache_dir: String::new(),
            mirror: String::new(),
        }
    }

    /// The defaults are applied first, then the descriptions, then the
    /// permissions; only the descriptions and the permissions are checked.
    #[test]
    fn the_defaults_are_applied_first_and_not_checked() {
        let defaults = config::defaults(&[fragment("image", "BR2_PACKAGE_BUSYBOX=y\n")]);
        let prepared =
            prepared_for_test(vec![fragment("image", "BR2_PACKAGE_BUSYBOX=y\n")], defaults);
        let (applied, checked) = merged_fragments(&prepared);
        assert_eq!(applied[0].name, "gaia.toolchain");
        assert_eq!(applied.len(), checked.len() + 1);
        assert_eq!(
            applied.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["gaia.toolchain", "image", "gaia.permissions"]
        );
        assert!(
            checked.iter().all(|f| f.name != "gaia.toolchain"),
            "the defaults are not checked: {:?}",
            checked.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
        assert_eq!(
            checked.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["image", "gaia.permissions"]
        );
    }

    /// A line the resolved config dropped is reported for a description, and
    /// is not for the defaults: the checked list has no default in it.
    #[test]
    fn a_default_line_buildroot_drops_is_not_reported() {
        let prepared = prepared_for_test(
            vec![fragment("board.target", "BR2_PACKAGE_NO_SUCH=y\n")],
            config::defaults(&[]),
        );
        let (_, checked) = merged_fragments(&prepared);
        // The resolved config has none of the defaults' lines: the
        // architecture does not fit the variant, so Kconfig dropped them.
        let reported = config::not_taken("BR2_PACKAGE_BUSYBOX=y\n", &checked);
        assert!(
            reported
                .iter()
                .all(|line| !line.starts_with("gaia.toolchain")),
            "a default line was reported: {reported:?}"
        );
        assert!(
            reported
                .iter()
                .any(|line| line.starts_with("board.target: `BR2_PACKAGE_NO_SUCH=y` was dropped")),
            "the description's line is reported: {reported:?}"
        );
    }

    /// The key changes with the defaults and without them, and a build that
    /// chooses its toolchain has the same key whether or not defaults are
    /// computed for it.
    #[test]
    fn the_key_changes_with_the_defaults_and_not_for_a_chosen_toolchain() {
        let descriptions = vec![fragment("image", "BR2_PACKAGE_BUSYBOX=y\n")];
        let with = prepared_for_test(descriptions.clone(), config::defaults(&descriptions));
        let without = prepared_for_test(descriptions.clone(), Vec::new());
        assert_ne!(key_of(&with), key_of(&without));
        // The same defaults text keys the same.
        let again = prepared_for_test(descriptions.clone(), config::defaults(&descriptions));
        assert_eq!(key_of(&with), key_of(&again));

        // A chosen toolchain gets no defaults, so its key is the key it had
        // before the defaults existed: the same as a build with none.
        let chosen = vec![fragment("board", "BR2_TOOLCHAIN_BUILDROOT=y\n")];
        let chosen_defaults = config::defaults(&chosen);
        assert!(chosen_defaults.is_empty());
        assert_eq!(
            key_of(&prepared_for_test(chosen.clone(), chosen_defaults)),
            key_of(&prepared_for_test(chosen, Vec::new()))
        );
    }
}

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
            download_pool: String::new(),
            ccache_dir: String::new(),
            mirror: String::new(),
            config_only: false,
            dev_reuse: String::new(),
            bwrap: String::new(),
        }
    }

    /// The compiler cache's directory and the mirror are make variables on
    /// the command line, given only when they are set.
    #[test]
    fn the_ccache_directory_and_the_mirror_are_make_variables() {
        let mut host = host(4, "");
        let bare = make_arguments(&host, Path::new("/out"), &[], &[]);
        assert!(bare.iter().all(|arg| !arg.starts_with("BR2_CCACHE_DIR=")));
        assert!(bare.iter().all(|arg| !arg.starts_with("BR2_PRIMARY_SITE")));
        host.ccache_dir = "/store/buildroot-ccache".into();
        host.mirror = "https://mirror.example/br".into();
        let set = make_arguments(&host, Path::new("/out"), &[], &["all"]);
        assert!(set.contains(&"BR2_CCACHE_DIR=/store/buildroot-ccache".to_string()));
        assert!(set.contains(&"BR2_PRIMARY_SITE=https://mirror.example/br".to_string()));
        // The upstream site is the fallback: the variable that would switch
        // it off is never given.
        assert!(set.iter().all(|arg| !arg.contains("PRIMARY_SITE_ONLY")));
        assert_eq!(set.last().map(String::as_str), Some("all"));
    }

    /// The key of a build holds the compiler cache as on or off, and neither
    /// its directory nor the mirror: they change where the cache lives and
    /// where sources come from, and the hashes pin what is built.
    #[test]
    fn the_key_has_neither_the_compiler_cache_directory_nor_the_mirror() {
        let fragment = |text: &str| ConfigFragment {
            name: "gaia.ccache".into(),
            text: text.into(),
        };
        let prepared = Prepared {
            src: PathBuf::from("/store/buildroot"),
            src_digest: Digest::of_bytes(b"source"),
            dirs: Vec::new(),
            paths: Vec::new(),
            vars: Vec::new(),
            fragments: vec![fragment("BR2_CCACHE=y\n")],
            defaults: Vec::new(),
            contracts: Vec::new(),
            patch_key: None,
        };
        let on = OsConfig {
            fragments: vec![fragment("BR2_CCACHE=y\n")],
            ..OsConfig::default()
        };
        let mut first = host(4, "");
        first.ccache_dir = "/store-a/buildroot-ccache".into();
        let mut second = host(4, "");
        second.ccache_dir = "/store-b/buildroot-ccache".into();
        second.mirror = "https://mirror.example/br".into();
        let key = |host: &BuildrootHost| prepared.key("test", 1, &on, host).finish();
        assert!(key(&first) == key(&second));
        let off = OsConfig {
            fragments: vec![fragment("# BR2_CCACHE is not set\n")],
            ..OsConfig::default()
        };
        assert!(prepared.key("test", 1, &off, &first).finish() != key(&first));
    }

    #[test]
    fn a_mirror_must_be_a_uri_with_no_white_space_or_quotes() {
        assert!(check_mirror("https://mirror.example/buildroot").is_ok());
        assert!(check_mirror("file:///srv/dl").is_ok());
        for bad in [
            "mirror.example",
            "https://a b",
            "https://a\"b",
            "https://$(X)",
        ] {
            assert!(check_mirror(bad).is_err(), "{bad:?}");
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
                "PARALLEL_JOBS=13",
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

    /// Every make runs with the fixed PATH, after the deterministic
    /// archivers (`tools`) in its output directory, and nothing inherited:
    /// the host tools Buildroot's `check-host-*` rules look for are in its
    /// dirs, and no entry is empty, relative or ".", which Buildroot refuses.
    /// No module it imports writes bytecode.
    #[test]
    fn make_runs_with_the_fixed_path_only() {
        let pool = jobs::Jobserver::new(4, false).expect("a pool");
        let slot = pool.admit(&|| false).expect("a slot");
        let tool = make_tool(
            &host(4, ""),
            Path::new("/src"),
            Path::new("/out"),
            &[],
            &[],
            &slot,
        );
        let debug = format!("{tool:?}");
        assert!(
            debug.contains(r#"("PATH", "/out/.gaia-tools:/usr/local/bin:/usr/bin:/bin")"#),
            "{debug}"
        );
        assert!(
            debug.contains(r#"("PYTHONDONTWRITEBYTECODE", "1")"#),
            "{debug}"
        );
        // A make stuck asking again for ever is stopped (`REPEATS`).
        assert!(
            debug.contains(&format!("repeated: Some(({REPEATS}, ")),
            "{debug}"
        );
        assert_eq!(debug.matches(r#"("PATH", "#).count(), 1, "{debug}");
        let entries: Vec<&str> = PATH.split(':').collect();
        assert!(entries.iter().all(|dir| dir.starts_with('/')), "{PATH}");
        assert!(entries.contains(&"/usr/bin"), "{PATH}");
        assert!(!PATH.contains(' ') && !PATH.contains('\t'), "{PATH}");
    }

    /// Every make names the fixed kernel build user and host, not the
    /// machine's, and a build time.
    #[test]
    fn make_names_no_builder() {
        let pool = jobs::Jobserver::new(4, false).expect("a pool");
        let slot = pool.admit(&|| false).expect("a slot");
        let tool = make_tool(
            &host(4, ""),
            Path::new("/src"),
            Path::new("/out"),
            &[],
            &[],
            &slot,
        );
        let debug = format!("{tool:?}");
        assert!(
            debug.contains(r#"("KBUILD_BUILD_USER", "gaia")"#),
            "{debug}"
        );
        assert!(
            debug.contains(r#"("KBUILD_BUILD_HOST", "gaia")"#),
            "{debug}"
        );
        assert!(
            debug.contains(r#"("KBUILD_BUILD_TIMESTAMP", "Thu Jan  1 00:00:00 UTC 1970")"#),
            "{debug}"
        );
        assert!(debug.contains(r#"("SOURCE_DATE_EPOCH", "0")"#), "{debug}");
        assert!(debug.contains(r#"("TZ", "UTC")"#), "{debug}");
    }

    /// A configuration that forces host builds is refused; the option off,
    /// or merely mentioned, is not.
    #[test]
    fn a_forced_host_build_is_seen_in_the_resolved_config() {
        assert!(forces_host_build("BR2_FORCE_HOST_BUILD=y\nBR2_x=y\n"));
        assert!(forces_host_build("  BR2_FORCE_HOST_BUILD=y\n"));
        assert!(!forces_host_build("# BR2_FORCE_HOST_BUILD is not set\n"));
        assert!(!forces_host_build("BR2_PER_PACKAGE_DIRECTORIES=y\n"));
        assert!(!forces_host_build(""));
    }

    /// With the pool's fifo, a build's make has the jobserver in `MAKEFLAGS`
    /// and no job count on its command line; the job count is what Buildroot's
    /// package makes inherit, so they add no `-j` of their own.
    #[test]
    fn a_build_with_the_pool_passes_the_jobserver_and_no_job_count() {
        let pool = jobs::Jobserver::new(13, true).expect("a pool");
        let slot = pool.admit(&|| false).expect("a slot");
        let tool = make_tool(
            &host(13, ""),
            Path::new("/src"),
            Path::new("/out"),
            &[],
            &["world"],
            &slot,
        );
        let debug = format!("{tool:?}");
        assert!(
            debug.contains(r#"("MAKEFLAGS", "-j --jobserver-auth=fifo:/proc/"#),
            "{debug}"
        );
        assert!(!debug.contains("-j13"), "{debug}");
        let plain = make_arguments(&host(13, ""), Path::new("/out"), &[], &["world"]);
        assert!(plain.contains(&"-j13".to_string()));
        let shared = arguments(&host(13, ""), Path::new("/out"), &[], &["world"], true);
        assert!(
            !shared.iter().any(|arg| arg.starts_with("-j")),
            "{shared:?}"
        );
        // The builds that take a job count of their own get this build's,
        // with the pool or without.
        for args in [&plain, &shared] {
            assert!(args.contains(&"PARALLEL_JOBS=13".to_string()), "{args:?}");
        }
    }

    /// The plant: a pool make that cannot open its jobserver (here a fifo
    /// that is not there, as the sandbox once could not open the pool's)
    /// fails the step as make says so, and is not left to build with one
    /// job. The same make with the pool's own `MAKEFLAGS` runs.
    #[test]
    fn a_pool_make_that_loses_the_jobserver_fails_at_once() {
        if !jobs::make_has_fifo_jobserver("/usr/bin/make") {
            eprintln!("skipped: /usr/bin/make is older than 4.4");
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("src");
        fs::create_dir_all(&src).expect("src");
        fs::write(src.join("Makefile"), "slow:\n\tsleep 30\nquick:\n\ttrue\n").expect("Makefile");
        let store = Store::open(dir.path().join("store")).expect("store");
        let cx = Ctx::new();
        let pool = jobs::Jobserver::new(13, true).expect("a pool");
        let slot = pool.admit(&|| false).expect("a slot");
        let host = host(13, "");
        let out = dir.path().join("out");
        let run =
            |name: &str, tool: Tool| {
                Step::new(&store, &cx, "test.make", KeyBuilder::new(name, 1).finish())
                    .look_up_or_make(&[("out", Path::new("out"))], |work| {
                        work.run(&tool)?;
                        fs::write(work.dir().join("out"), "x").map_err(|e| StepError::body("t", e))
                    })
            };
        let started = std::time::Instant::now();
        let lost = make_tool(&host, &src, &out, &[], &["slow"], &slot).env(
            "MAKEFLAGS",
            "-j --jobserver-auth=fifo:/nonexistent/gaia-planted",
        );
        let error = run("lost", lost).expect_err("the jobserver is lost");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "make ran on for {:?}",
            started.elapsed()
        );
        let ErrorKind::Body(why) = error.kind() else {
            panic!("{error}");
        };
        assert!(
            why.contains(JOBSERVER_WHY) && why.contains("jobserver"),
            "{why}"
        );
        run("kept", make_tool(&host, &src, &out, &[], &["quick"], &slot))
            .expect("the pool's own jobserver");
    }
}
