//! `gaia.build.node`: a Node.js package built into a bundle, in a pinned
//! glibc image, through bubblewrap.
//!
//! - **Sandboxed:** `npm` runs by [`gaia_sandbox`], with the image's root
//!   filesystem (`root`, a pulled image tree) as the root. Nothing of the
//!   host is there but what is bound: the Node.js install at `/toolchain`
//!   (read-only) and the step's directory at `/build`, the only writable
//!   path. The source and the npm cache are copied there first, since npm
//!   writes into both. The network is off.
//! - **Pinned Node.js:** the install at the path given (`bin/node`,
//!   `bin/npm`), a self-contained one such as an upstream release;
//!   `node --version` and `npm --version`, run in the same sandbox, are in
//!   the key. npm runs with only the environment set here, its own `HOME`
//!   and cache.
//! - **Offline, from the lock:** `npm ci --offline` installs exactly what
//!   `package-lock.json` names, from the `cache` tree: an npm cache
//!   (`npm cache add` of each locked tarball) that a source node fills from
//!   the lock. A source without `package-lock.json` is refused. Dependency
//!   install scripts do not run (`--ignore-scripts`), so a dependency that
//!   needs a native build is not supported here.
//! - **The build:** the package's own script (`npm run <script>`), when
//!   one is named, then `npm prune --omit=dev`.
//! - **Reproducible:** `SOURCE_DATE_EPOCH` is 0; the store keeps content
//!   and modes, not times, and npm writes no time into what it installs.
//!
//! Outputs: the bundle (the package with its built files and production
//! `node_modules`) as a tree, and the package's `npm pack` tarball as an
//! artifact.

use daedalus::prelude::*;
use gaia_sandbox::Sandbox;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.build.node";
/// The probe that reads a tool's version inside the sandbox.
const PROBE: &str = "gaia.build.node.version";
/// Changes when how the build is run changes.
const RECIPE_VERSION: u32 = 2;

/// Where the sandbox binds the toolchain and the writable step directory.
const TOOLCHAIN: &str = "/toolchain";
const BUILD: &str = "/build";
const PATH_IN_SANDBOX: &str = "/toolchain/bin:/usr/bin:/bin";

/// Installs `source`'s locked dependencies from `cache` with the Node.js at
/// `node` (a directory on this host, bound read-only), runs its `script`
/// (none when empty) and prunes dev dependencies, in the image `root` (a
/// stored root filesystem tree). `bwrap` is the bubblewrap program that
/// runs every tool.
#[node(
    id = "gaia.build.node",
    inputs(store, bwrap, root, node, source, cache, script),
    outputs(bundle, package)
)]
#[allow(clippy::too_many_arguments)]
pub fn build_node(
    store: &str,
    bwrap: &str,
    root: Tree,
    node: &str,
    source: Tree,
    cache: Tree,
    script: &str,
    cx: &Ctx,
) -> Result<(Tree, Artifact), StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let fail = |message: String| StepError::body(NAME, message);
    let dir_of = |tree: &Tree, what: &str| {
        let object = tree
            .object
            .object_ref()
            .ok_or_else(|| fail(format!("the {what} is not in the store")))?;
        Ok::<_, StepError>((object.digest, store.path(&object)))
    };
    let (root_digest, root_dir) = dir_of(&root, "image")?;
    let (source_digest, source_dir) = dir_of(&source, "source")?;
    let (cache_digest, cache_dir) = dir_of(&cache, "npm cache tree")?;
    if !source_dir.join("package-lock.json").is_file() {
        return Err(fail(
            "the source has no package-lock.json; the install is locked".into(),
        ));
    }
    // What every tool of this build sees: the image and Node.js. The
    // writable step directory is added for each step.
    let base = Sandbox::new(bwrap, root_dir)
        .input(PathBuf::from(node), TOOLCHAIN)
        .env("PATH", PATH_IN_SANDBOX);
    let version = |program: &str| -> Result<String, StepError> {
        let key = base
            .key(KeyBuilder::new(PROBE, 1).config("program", program))
            .finish();
        let made = Step::new(&store, cx, PROBE, key).look_up_or_make(
            &[("version", Path::new("version"))],
            |work| {
                let tool = base
                    .clone()
                    .output(work.dir(), BUILD)
                    .tool("/bin/sh")
                    .map_err(|e| fail(format!("the sandbox: {e}")))?
                    .args(["-c", "exec \"$0\" --version >/build/version 2>&1"])
                    .arg(format!("{TOOLCHAIN}/bin/{program}"));
                work.run(&tool).map(|_| ())
            },
        )?;
        std::fs::read_to_string(store.path(&made.outputs["version"]))
            .map_err(|e| fail(format!("{program} --version: {e}")))
    };
    let (node_version, npm_version) = (version("node")?, version("npm")?);
    let sandbox = base
        .env("HOME", format!("{BUILD}/home"))
        .env("SOURCE_DATE_EPOCH", "0")
        .env("TZ", "UTC")
        .env("LC_ALL", "C")
        .env("NODE_ENV", "production")
        .cwd(format!("{BUILD}/bundle"));
    let key = sandbox
        .key(
            KeyBuilder::new(NAME, RECIPE_VERSION)
                .config("node", node_version)
                .config("npm", npm_version)
                .config("script", script)
                .input("image", root_digest)
                .input("source", source_digest)
                .input("cache", cache_digest),
        )
        .finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[
            ("bundle", Path::new("bundle")),
            ("package", Path::new("package.tgz")),
        ],
        |work| {
            let io = |e: std::io::Error| fail(e.to_string());
            let build = work.dir().to_path_buf();
            let packed = build.join("packed");
            copy_writable(&source_dir, &build.join("bundle")).map_err(io)?;
            copy_writable(&cache_dir, &build.join("cache")).map_err(io)?;
            for dir in [&build.join("home"), &packed] {
                std::fs::create_dir_all(dir).map_err(io)?;
            }
            let sandbox = sandbox.clone().output(&build, BUILD);
            let npm = |args: &[&str]| {
                Ok::<_, StepError>(
                    sandbox
                        .tool(&format!("{TOOLCHAIN}/bin/npm"))
                        .map_err(|e| fail(format!("the sandbox: {e}")))?
                        .args(args)
                        .args([
                            "--offline",
                            "--no-audit",
                            "--no-fund",
                            "--no-update-notifier",
                        ])
                        .arg("--cache")
                        .arg(format!("{BUILD}/cache")),
                )
            };
            work.run(&npm(&["ci", "--ignore-scripts", "--include=dev"])?)?;
            if !script.is_empty() {
                work.run(&npm(&["run", script])?)?;
            }
            work.run(&npm(&["prune", "--omit=dev", "--ignore-scripts"])?)?;
            let pack = npm(&["pack", "--ignore-scripts"])?
                .arg("--pack-destination")
                .arg(format!("{BUILD}/packed"));
            work.run(&pack)?;
            let mut tarballs: Vec<_> = std::fs::read_dir(&packed)
                .map_err(io)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .collect();
            if tarballs.len() != 1 {
                return Err(fail("npm pack made no single tarball".into()));
            }
            std::fs::rename(tarballs.remove(0), build.join("package.tgz")).map_err(io)
        },
    )?;
    Ok((
        Tree {
            object: made.outputs["bundle"].into(),
            entries: Vec::new(),
        },
        Artifact {
            kind: ArtifactKind::Package,
            object: made.outputs["package"].into(),
        },
    ))
}

/// Copies `from` to `to` (which must not exist), every file and directory
/// writable by its owner, symlinks as links.
fn copy_writable(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir(to)?;
    std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o755))?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if kind.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&src)?, &dst)?;
        } else if kind.is_dir() {
            copy_writable(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
            let mode = std::fs::metadata(&src)?.permissions().mode() | 0o200;
            std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(mode & 0o7777))?;
        }
    }
    Ok(())
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&BUILD_NODE_NODE];
