//! `gaia.build.make`: a build run by the component's own Makefile, in the
//! component's pinned image, sandboxed and offline.
//!
//! For software built with a C compiler (or anything `make` drives) that
//! no other builder takes. The component gives the Makefile, in its build
//! source, and names the other sources the build reads; Gaia gives the
//! rest and nothing more:
//!
//! - **The image** (`root`, a pulled image tree) is the root, read-only:
//!   its `make`, compiler and libraries are the ones used, and nothing of
//!   this machine. No network.
//! - **The sources:** the build source (with the Makefile) at `/src`, each
//!   other source at `/deps/<name>`, read-only.
//! - **The Makefile is run** as `make -f /src/Makefile [targets]` in a
//!   writable, empty `/work/build`, with `SRC=/src`, `DEPS=/deps`,
//!   `OUT=/work/out`, `SOURCE_DATE_EPOCH=0`, `LC_ALL=C`, `PATH=/usr/bin:/bin`,
//!   and `HOME` and `TMPDIR` of its own. What it puts under `$(OUT)` is the
//!   step's output, a tree, each file of it also an artifact; the build
//!   directory is dropped. An empty `$(OUT)` fails the step.
//!
//! The key holds the image's and every source's digest with its name, the
//! targets and what the sandbox adds.

use daedalus::prelude::*;
use daedalus_cpu_author::FanIn;
use gaia_sandbox::Sandbox;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, ArtifactKind, Tree};
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.build.make";
/// Changes when how the build is run changes.
const RECIPE_VERSION: u32 = 1;
const SOURCE: &str = "/src";
const DEPS: &str = "/deps";
const WORK: &str = "/work";

/// Runs `/src/Makefile` of `source` with `targets` (words separated by
/// spaces; empty for the Makefile's first) in the image `root`, the trees
/// of `deps` bound at `/deps/<name>` by the names in `dep_names`
/// (separated by spaces, in the same order). `bwrap` runs it.
#[node(
    id = "gaia.build.make",
    inputs(store, bwrap, root, source, deps, dep_names, targets),
    outputs(tree, artifacts)
)]
#[allow(clippy::too_many_arguments)]
pub fn build_make(
    store: &str,
    bwrap: &str,
    root: Tree,
    source: Tree,
    deps: FanIn<'_, Tree>,
    dep_names: Option<&String>,
    targets: Option<&String>,
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
    let (source_digest, source_dir) = dir_of(&source, "build source")?;
    if !source_dir.join("Makefile").is_file() {
        return Err(fail("the build source has no Makefile at its top".into()));
    }
    let names: Vec<&str> = dep_names
        .map(String::as_str)
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    let trees: Vec<Tree> = deps.iter().map(|t| (*t).clone()).collect();
    if names.len() != trees.len() {
        return Err(fail(format!(
            "{} sources are given and {} names for them",
            trees.len(),
            names.len()
        )));
    }
    for name in &names {
        let plain = !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !plain {
            return Err(fail(format!(
                "`{name}` is not a source name: letters, digits, - and _"
            )));
        }
    }
    let targets: Vec<&str> = targets
        .map(String::as_str)
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    if let Some(bad) = targets.iter().find(|t| t.starts_with('-')) {
        return Err(fail(format!(
            "target `{bad}` is a flag; the Makefile sets its own flags"
        )));
    }
    let mut sandbox = Sandbox::new(bwrap, root_dir.clone()).input(source_dir.clone(), SOURCE);
    let mut key = KeyBuilder::new(NAME, RECIPE_VERSION)
        .input("root", root_digest)
        .input("source", source_digest)
        .config("targets", targets.join(" "));
    for (name, tree) in names.iter().zip(&trees) {
        let (digest, dir) = dir_of(tree, &format!("source {name}"))?;
        sandbox = sandbox.input(dir, format!("{DEPS}/{name}"));
        key = key.input(&format!("deps.{name}"), digest);
    }
    let sandbox = sandbox
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .env("SOURCE_DATE_EPOCH", "0")
        .env("SRC", SOURCE)
        .env("DEPS", DEPS)
        .env("OUT", format!("{WORK}/out"))
        .env("HOME", format!("{WORK}/home"))
        .env("TMPDIR", "/tmp")
        .cwd(format!("{WORK}/build"));
    let key = sandbox.key(key).finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("out", Path::new("work/out"))],
        |work| {
            let io = |e: std::io::Error| StepError::body(NAME, e.to_string());
            let dir = work.dir().join("work");
            for sub in ["build", "out", "home"] {
                std::fs::create_dir_all(dir.join(sub)).map_err(io)?;
            }
            let mut make = sandbox
                .clone()
                .output(&dir, WORK)
                .tool("/usr/bin/make")
                .map_err(io)?
                .args(["-f", "/src/Makefile"]);
            for target in &targets {
                make = make.arg(target);
            }
            if !root_dir.join("usr/bin/make").exists() {
                return Err(StepError::body(
                    NAME,
                    "the image has no /usr/bin/make: a make build runs the image's own",
                ));
            }
            work.run(&make)?;
            for sub in ["build", "home"] {
                std::fs::remove_dir_all(dir.join(sub)).map_err(io)?;
            }
            if std::fs::read_dir(dir.join("out"))
                .map_err(io)?
                .next()
                .is_none()
            {
                return Err(StepError::body(
                    NAME,
                    "the Makefile put nothing under $(OUT)",
                ));
            }
            Ok(())
        },
    )?;
    let out = made.outputs["out"];
    let mut files = Vec::new();
    walk(&store.path(&out), &mut files).map_err(|e| fail(e.to_string()))?;
    files.sort();
    let artifacts = files
        .iter()
        .map(|f| {
            let object = store.put_file(f).map_err(StepError::store(NAME))?;
            Ok(Artifact {
                kind: ArtifactKind::Binary,
                object: object.into(),
            })
        })
        .collect::<Result<_, StepError>>()?;
    Ok((
        Tree {
            object: out.into(),
            entries: Vec::new(),
        },
        artifacts,
    ))
}

/// Every regular file under `dir`.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            walk(&entry.path(), out)?;
        } else if kind.is_file() {
            out.push(entry.path());
        }
    }
    Ok(())
}

/// The node, for a registry.
pub const NODES: [&NodeRecord; 1] = [&BUILD_MAKE_NODE];
