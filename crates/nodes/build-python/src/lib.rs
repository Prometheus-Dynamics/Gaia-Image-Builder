//! `gaia.build.python`: a Python project built into its wheel and the tree
//! it installs to.
//!
//! - **Pinned interpreter:** the Python program given. Its `-VV` and the
//!   distributions it has (the build backend among them) are in the key.
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
//!   copy at a fixed place in the step's directory, and installed scripts
//!   start `#!<interpreter>`, the target's Python, not the build host's.
//! - **Consistent records:** rewriting a script changes its bytes, so each
//!   installed `.dist-info/RECORD` gets the new hash and size of every file
//!   it lists. The rows are otherwise kept as pip wrote them.
//!
//! The outputs are a tree with `wheels/<the built wheel>` and `site/` (the
//! installed packages and `bin/` scripts, as `pip install --target` lays
//! them out), and the built wheel as an artifact.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store, Tool};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.build.python";
/// Changes when how the build is run changes. 2: RECORD follows the
/// rewritten scripts.
const RECIPE_VERSION: u32 = 2;
/// 1980-01-01, the earliest time a zip holds.
const EPOCH: &str = "315532800";

/// Builds the project in `source` with the Python at `python`, and
/// installs it with the wheels in `wheels` for `platform`; scripts run
/// `interpreter`.
#[node(
    id = "gaia.build.python",
    inputs(store, python, source, wheels, platform, interpreter),
    outputs(tree, artifacts)
)]
pub fn build_python(
    store: &str,
    python: &str,
    source: Tree,
    wheels: Tree,
    platform: &str,
    interpreter: &str,
    cx: &Ctx,
) -> Result<(Tree, Vec<Artifact>), StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let dir_of = |tree: &Tree, what: &str| {
        let object = tree
            .object
            .object_ref()
            .ok_or_else(|| StepError::body(NAME, format!("the {what} is not in the store")))?;
        Ok::<_, StepError>((object.digest, store.path(&object)))
    };
    let (source_digest, source_dir) = dir_of(&source, "source")?;
    let (wheels_digest, wheels_dir) = dir_of(&wheels, "dependency wheels tree")?;
    if !source_dir.join("pyproject.toml").is_file() && !source_dir.join("setup.py").is_file() {
        return Err(StepError::body(
            NAME,
            "the source has no pyproject.toml or setup.py",
        ));
    }
    if !interpreter.starts_with('/') {
        return Err(StepError::body(
            NAME,
            format!("the interpreter {interpreter:?} is not an absolute path"),
        ));
    }
    let python = Path::new(python);
    let ask = |args: &[&str]| {
        std::process::Command::new(python)
            .args(args)
            .env_clear()
            .output()
            .map_err(|e| StepError::body(NAME, format!("{}: {e}", python.display())))
            .map(|o| o.stdout)
    };
    let key = KeyBuilder::new(NAME, RECIPE_VERSION)
        .config("python", ask(&["-VV"])?)
        .config(
            "distributions",
            ask(&[
                "-c",
                "import importlib.metadata as m\nfor d in sorted({(d.metadata['Name'] or '').lower()+' '+d.version for d in m.distributions()}): print(d)",
            ])?,
        )
        .config("platform", platform)
        .config("interpreter", interpreter)
        .input("source", source_digest)
        .input("wheels", wheels_digest)
        .finish();
    let made =
        Step::new(&store, cx, NAME, key).look_up_or_make(&[("out", Path::new("out"))], |work| {
            let dir = work.dir().to_path_buf();
            let fail = |e: std::io::Error| StepError::body(NAME, e);
            let src = dir.join("src");
            copy_dir(&source_dir, &src).map_err(fail)?;
            let (built, site, tmp) = (
                dir.join("out/wheels"),
                dir.join("out/site"),
                dir.join("tmp"),
            );
            for d in [&built, &tmp, &dir.join("home")] {
                std::fs::create_dir_all(d).map_err(fail)?;
            }
            let pip = |args: &[&str]| {
                Tool::new(python)
                    .args(["-m", "pip"])
                    .args(args.iter().copied())
                    .env("PATH", "/usr/bin:/bin")
                    .env("HOME", dir.join("home"))
                    .env("TMPDIR", &tmp)
                    .env("SOURCE_DATE_EPOCH", EPOCH)
                    .env("PYTHONDONTWRITEBYTECODE", "1")
                    .env("PYTHONHASHSEED", "0")
                    .env("PIP_NO_INPUT", "1")
                    .env("PIP_NO_CACHE_DIR", "1")
                    .env("PIP_DISABLE_PIP_VERSION_CHECK", "1")
                    .env("PIP_CONFIG_FILE", "/dev/null")
                    .cwd(&src)
            };
            work.run(
                &pip(&[
                    "wheel",
                    "--no-build-isolation",
                    "--no-index",
                    "--no-deps",
                    "--wheel-dir",
                ])
                .arg(&built)
                .arg(&src),
            )?;
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
            ])
            .arg("--find-links")
            .arg(&built)
            .arg("--find-links")
            .arg(&wheels_dir)
            .arg("--target")
            .arg(&site);
            if !platform.is_empty() {
                install = install.args(["--platform", platform, "--only-binary=:all:"]);
            }
            work.run(&install.arg(&project))?;
            reshebang(&site.join("bin"), interpreter).map_err(fail)?;
            rerecord(&site).map_err(fail)?;
            for d in [&src, &tmp, &dir.join("home")] {
                std::fs::remove_dir_all(d).map_err(fail)?;
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

/// Gives each `.dist-info/RECORD` directly under `site` the hash and size
/// of every file it lists, as the files are now. A row with no hash (the
/// RECORD's own row) is kept as it is.
fn rerecord(site: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(site)? {
        let info = entry?.path();
        if !info.is_dir() || !info.extension().is_some_and(|x| x == "dist-info") {
            continue;
        }
        let record = info.join("RECORD");
        let text = std::fs::read_to_string(&record)?;
        let mut out = String::with_capacity(text.len());
        for line in text.lines().filter(|l| !l.is_empty()) {
            let (path, rest) = split_record_row(line).ok_or_else(|| {
                std::io::Error::other(format!("{}: bad row {line:?}", record.display()))
            })?;
            if rest.split(',').next().unwrap_or_default().is_empty() {
                out.push_str(line);
            } else {
                let file = record_file(site, &path);
                let digest = gaia_store::file_digest(&file)
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
                let size = std::fs::metadata(&file)?.len();
                out.push_str(&format!(
                    "{},sha256={},{size}",
                    quote_record_path(&path),
                    URL_SAFE_NO_PAD.encode(digest.as_bytes())
                ));
            }
            out.push('\n');
        }
        std::fs::write(&record, out)?;
    }
    Ok(())
}

/// The file a RECORD row names, under `site`. pip writes a target's scripts
/// as paths that climb out of it (`../../bin/x`), and the file is `bin/x`
/// under `site`, so the leading `../` are dropped.
fn record_file(site: &Path, path: &str) -> PathBuf {
    site.join(path.trim_start_matches("../"))
}

/// The path and the rest of a RECORD row (`hash,size`), read as CSV: a path
/// with a comma or a quote is quoted, and a quote inside it is doubled.
fn split_record_row(line: &str) -> Option<(String, &str)> {
    let Some(quoted) = line.strip_prefix('"') else {
        let (path, rest) = line.split_once(',')?;
        return Some((path.to_string(), rest));
    };
    let mut path = String::new();
    let mut chars = quoted.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if c != '"' {
            path.push(c);
        } else if matches!(chars.peek(), Some((_, '"'))) {
            path.push('"');
            chars.next();
        } else {
            return Some((path, quoted.get(at + 1..)?.strip_prefix(',')?));
        }
    }
    None
}

/// A path as a RECORD row writes it: quoted if it needs quotes.
fn quote_record_path(path: &str) -> String {
    if path.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", path.replace('"', "\"\""))
    } else {
        path.to_string()
    }
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
