//! `gaia.build.python`: a Python project built into its wheel and the tree
//! it installs to, in a pinned glibc image, through bubblewrap.
//!
//! - **Sandboxed:** pip runs by [`gaia_sandbox`], with the image's root
//!   filesystem (`root`, a pulled image tree) as the root. Nothing of the
//!   host is there but what is bound: the Python install at `/toolchain`
//!   and the dependency wheels at `/wheels` (both read-only), and the
//!   step's directory at `/build`, the only writable path. The source is
//!   copied there first, since a build backend writes beside it. The
//!   network is off.
//! - **Pinned interpreter:** the Python install given (a prefix directory
//!   with `bin/python3`, whose libraries are under its `lib`). Its `-VV`
//!   and the distributions it has (the build backend among them), read in
//!   the same sandbox, are in the key.
//! - **Offline:** the wheel is built with `pip wheel --no-build-isolation
//!   --no-index --no-deps`, so the build backend is the interpreter's own,
//!   and installed with `pip install --no-index --find-links` over the
//!   built wheel and the dependency wheels from the lock. Nothing is
//!   fetched.
//! - **For a target:** `platform` (a pip platform tag such as
//!   `manylinux2014_aarch64`; empty for the build host) picks which binary
//!   wheels the install takes, and only binary wheels are installed then.
//! - **Reproducible:** `SOURCE_DATE_EPOCH` fixes the wheel's timestamps,
//!   no bytecode is written (`--no-compile`), the project is built from a
//!   copy at a fixed path (`/build/src`), and installed scripts
//!   start `#!<interpreter>`, the target's Python, not the build host's.
//!
//! The outputs are a tree with `wheels/<the built wheel>` and `site/` (the
//! installed packages and `bin/` scripts, as `pip install --target` lays
//! them out), and the built wheel as an artifact.

use daedalus::prelude::*;
use gaia_sandbox::Sandbox;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.build.python";
/// The probe that asks the interpreter about itself inside the sandbox.
const PROBE: &str = "gaia.build.python.probe";
/// Changes when how the build is run changes.
const RECIPE_VERSION: u32 = 2;

/// Where the sandbox binds each input and the writable step directory.
const TOOLCHAIN: &str = "/toolchain";
const PYTHON: &str = "/toolchain/bin/python3";
const WHEELS: &str = "/wheels";
const BUILD: &str = "/build";
const PATH_IN_SANDBOX: &str = "/toolchain/bin:/usr/bin:/bin";
/// 1980-01-01, the earliest time a zip holds.
const EPOCH: &str = "315532800";

/// Builds the project in `source` with the Python installed at `python`
/// (a prefix directory on this host, bound read-only), and installs it
/// with the wheels in `wheels` for `platform`; scripts run `interpreter`.
/// Every tool runs in the image `root` (a stored root filesystem tree), by
/// the bubblewrap program `bwrap`.
#[node(
    id = "gaia.build.python",
    inputs(store, bwrap, root, python, source, wheels, platform, interpreter),
    outputs(tree, artifacts)
)]
#[allow(clippy::too_many_arguments)]
pub fn build_python(
    store: &str,
    bwrap: &str,
    root: Tree,
    python: &str,
    source: Tree,
    wheels: Tree,
    platform: &str,
    interpreter: &str,
    cx: &Ctx,
) -> Result<(Tree, Vec<Artifact>), StepError> {
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
    let (wheels_digest, wheels_dir) = dir_of(&wheels, "dependency wheels tree")?;
    if !source_dir.join("pyproject.toml").is_file() && !source_dir.join("setup.py").is_file() {
        return Err(fail("the source has no pyproject.toml or setup.py".into()));
    }
    if !interpreter.starts_with('/') {
        return Err(fail(format!(
            "the interpreter {interpreter:?} is not an absolute path"
        )));
    }
    // What every tool of this build sees: the image, Python and the wheels.
    // The writable step directory is added for each step.
    let base = Sandbox::new(bwrap, root_dir)
        .input(PathBuf::from(python), TOOLCHAIN)
        .input(wheels_dir, WHEELS)
        .env("PATH", PATH_IN_SANDBOX)
        // A Python built with a shared libpython finds it here.
        .env("LD_LIBRARY_PATH", format!("{TOOLCHAIN}/lib"))
        .env("HOME", format!("{BUILD}/home"))
        .env("SOURCE_DATE_EPOCH", EPOCH)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONHASHSEED", "0")
        .env("PIP_NO_INPUT", "1")
        .env("PIP_NO_CACHE_DIR", "1")
        .env("PIP_DISABLE_PIP_VERSION_CHECK", "1")
        .env("PIP_CONFIG_FILE", "/dev/null");
    // The interpreter's version and distributions, written to a file by a
    // step of its own (the list can be longer than a tool's output tail).
    let about = {
        let key = base.key(KeyBuilder::new(PROBE, 1)).finish();
        let made = Step::new(&store, cx, PROBE, key).look_up_or_make(
            &[("about", Path::new("about"))],
            |work| {
                let tool = base
                    .clone()
                    .output(work.dir(), BUILD)
                    .tool(PYTHON)
                    .map_err(|e| fail(format!("the sandbox: {e}")))?
                    .arg("-c")
                    .arg(
                        "import importlib.metadata as m, sys\n\
                         names = sorted({(d.metadata['Name'] or '').lower() + ' ' + d.version for d in m.distributions()})\n\
                         open('/build/about', 'w').write(sys.version + '\\n' + '\\n'.join(names) + '\\n')",
                    );
                work.run(&tool).map(|_| ())
            },
        )?;
        std::fs::read_to_string(store.path(&made.outputs["about"]))
            .map_err(|e| fail(format!("{python}: {e}")))?
    };
    let sandbox = base.cwd(format!("{BUILD}/src"));
    let key = sandbox
        .key(
            KeyBuilder::new(NAME, RECIPE_VERSION)
                .config("python", about)
                .config("platform", platform)
                .config("interpreter", interpreter)
                .input("image", root_digest)
                .input("source", source_digest)
                .input("wheels", wheels_digest),
        )
        .finish();
    let made =
        Step::new(&store, cx, NAME, key).look_up_or_make(&[("out", Path::new("out"))], |work| {
            let dir = work.dir().to_path_buf();
            let io = |e: std::io::Error| fail(e.to_string());
            let src = dir.join("src");
            copy_dir(&source_dir, &src).map_err(io)?;
            let (built, site, home) = (
                dir.join("out/wheels"),
                dir.join("out/site"),
                dir.join("home"),
            );
            for d in [&built, &home] {
                std::fs::create_dir_all(d).map_err(io)?;
            }
            let sandbox = sandbox.clone().output(&dir, BUILD);
            let pip = |args: &[&str]| {
                Ok::<_, StepError>(
                    sandbox
                        .tool(PYTHON)
                        .map_err(|e| fail(format!("the sandbox: {e}")))?
                        .args(["-m", "pip"])
                        .args(args.iter().copied()),
                )
            };
            work.run(&pip(&[
                "wheel",
                "--no-build-isolation",
                "--no-index",
                "--no-deps",
                "--wheel-dir",
                &format!("{BUILD}/out/wheels"),
                &format!("{BUILD}/src"),
            ])?)?;
            let wheel = only_wheel(&built)?;
            let project = wheel
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.split('-').next())
                .unwrap_or_default()
                .to_string();
            let mut install = pip(&[
                "install",
                "--no-index",
                "--no-compile",
                "--no-warn-script-location",
                "--find-links",
                &format!("{BUILD}/out/wheels"),
                "--find-links",
                WHEELS,
                "--target",
                &format!("{BUILD}/out/site"),
            ])?;
            if !platform.is_empty() {
                install = install.args(["--platform", platform, "--only-binary=:all:"]);
            }
            work.run(&install.arg(&project))?;
            reshebang(&site.join("bin"), interpreter).map_err(io)?;
            for d in [&src, &home] {
                std::fs::remove_dir_all(d).map_err(io)?;
            }
            Ok(())
        })?;
    let out = made.outputs["out"];
    let wheel = only_wheel(&store.path(&out).join("wheels"))?;
    let object = store.put_file(&wheel).map_err(StepError::store(NAME))?;
    Ok((
        Tree {
            object: out.into(),
            entries: Vec::new(),
        },
        vec![Artifact {
            kind: ArtifactKind::Package,
            object: object.into(),
        }],
    ))
}

/// The one wheel in `dir`.
fn only_wheel(dir: &Path) -> Result<PathBuf, StepError> {
    let wheels: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| StepError::body(NAME, e))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "whl"))
        .collect();
    match wheels.as_slice() {
        [one] => Ok(one.clone()),
        _ => Err(StepError::body(
            NAME,
            format!("the build made {} wheels, not one", wheels.len()),
        )),
    }
}

/// Points each script's `#!` line at `interpreter`.
fn reshebang(bin: &Path, interpreter: &str) -> std::io::Result<()> {
    let Ok(entries) = std::fs::read_dir(bin) else {
        return Ok(());
    };
    for entry in entries {
        let path = entry?.path();
        let bytes = std::fs::read(&path)?;
        if bytes.starts_with(b"#!") {
            let end = bytes
                .iter()
                .position(|b| *b == b'\n')
                .unwrap_or(bytes.len());
            let mut out = format!("#!{interpreter}").into_bytes();
            out.extend_from_slice(&bytes[end..]);
            std::fs::write(&path, out)?;
        }
    }
    Ok(())
}

/// Copies a directory: files with their modes, symlinks as links.
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&src)?, &dst)?;
        } else if kind.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
            // Stored files are read-only; the build writes beside them.
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dst)?.permissions().mode();
            std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(mode | 0o200))?;
        }
    }
    Ok(())
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&BUILD_PYTHON_NODE];
