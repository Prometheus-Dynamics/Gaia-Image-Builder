//! `gaia.source.patch`: a source tree with patch series applied.

use crate::files::copy_tree;
use daedalus::prelude::*;
use daedalus_cpu_author::FanIn;
use gaia_step::{Ctx, Digest, KeyBuilder, Step, StepError, Store, Tool};
use gaia_types::Tree;
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.source.patch";
/// The work directory this step expects when no earlier run recorded its
/// size. In RAM when there is room (see `gaia_step::Step::ram_work`): a tree
/// of many small files is written once to the store's disk, not written,
/// read back and renamed there.
const WORK_BYTES: u64 = 2 << 30;
/// Changes when how the patches are applied changes.
const RECIPE_VERSION: u32 = 1;

/// `source` with the patches of `patches` applied in turn by the `patch`
/// program, strictly: `-p1 --fuzz=0`, every hunk where it says.
///
/// `paths` (JSON, a list of one path per tree of `patches`, in the same
/// order) names what in each tree is applied: a file is one patch; a
/// directory is the patches its `series` file lists, in that order, or
/// with no `series` its `*.patch` files in name order (as Buildroot and
/// quilt take a directory); an empty path is the whole tree as a
/// directory. A patch that does not apply fails the step, naming it.
///
/// The key holds the source's digest, each patch's bytes and name in
/// order, and the `patch` program's version.
#[node(
    id = "gaia.source.patch",
    inputs(store, patch, source, patches, paths),
    outputs(tree)
)]
pub fn source_patch(
    store: &str,
    patch: &str,
    source: Tree,
    patches: FanIn<'_, Tree>,
    paths: &str,
    cx: &Ctx,
) -> Result<Tree, StepError> {
    let fail = |why: String| StepError::body(NAME, why);
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let in_store = |tree: &Tree, what: &str| {
        tree.object
            .object_ref()
            .ok_or_else(|| fail(format!("{what} is not in the store")))
    };
    let source_object = in_store(&source, "the source")?;
    let paths: Vec<String> = serde_json::from_str(paths)
        .map_err(|e| fail(format!("the paths are not a list of text: {e}")))?;
    let trees: Vec<Tree> = patches.iter().map(|t| (*t).clone()).collect();
    if paths.len() != trees.len() {
        return Err(fail(format!(
            "{} patch trees are given and {} paths for them",
            trees.len(),
            paths.len()
        )));
    }
    let mut files = Vec::new();
    for (n, (tree, path)) in trees.iter().zip(&paths).enumerate() {
        let object = in_store(tree, &format!("patch tree {n}"))?;
        let root = store.path(&object);
        let at = if path.is_empty() {
            root.clone()
        } else {
            let relative = inside(path).map_err(fail)?;
            root.join(relative)
        };
        series(&at, path, &mut files).map_err(fail)?;
    }
    if files.is_empty() {
        return Err(fail("the patches given hold no patch".into()));
    }
    let version = std::process::Command::new(patch)
        .arg("--version")
        .env_clear()
        .output()
        .map_err(|e| fail(format!("{patch} --version: {e}")))?;
    let version = String::from_utf8_lossy(&version.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    let mut key = KeyBuilder::new(NAME, RECIPE_VERSION)
        .input("source", source_object.digest)
        .config("patch", version);
    for (n, (name, file)) in files.iter().enumerate() {
        let bytes = std::fs::read(file).map_err(|e| fail(format!("{name}: {e}")))?;
        key = key
            .input(&format!("patch {n}"), Digest::of_bytes(&bytes))
            .config(&format!("patch {n} name"), name);
    }
    let src = store.path(&source_object);
    let made = Step::new(&store, cx, NAME, key.finish())
        .ram_work(WORK_BYTES)
        .look_up_or_make(&[("tree", Path::new("tree"))], |work| {
            let tree = work.dir().join("tree");
            copy_tree(&src, &tree).map_err(|e| StepError::body(NAME, e.to_string()))?;
            let total = files.len();
            for (n, (name, file)) in files.iter().enumerate() {
                let tool = Tool::new(patch)
                    .args(["-p1", "--fuzz=0", "--batch", "--forward"])
                    .args(["--no-backup-if-mismatch", "--reject-file=-", "-s", "-i"])
                    .arg(file)
                    .cwd(&tree);
                work.run(&tool).map_err(|e| {
                    StepError::body(
                        NAME,
                        format!("patch {} of {total}, {name}, does not apply: {e}", n + 1),
                    )
                })?;
                work.progress(n as u32 + 1, total as u32);
            }
            Ok(())
        })?;
    Ok(Tree {
        object: made.outputs["tree"].into(),
        entries: Vec::new(),
    })
}

/// Adds the patches at `at` (`shown` as the board gave it) to `files`, each
/// with the name it is shown by.
fn series(at: &Path, shown: &str, files: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let shown = if shown.is_empty() { "." } else { shown };
    if at.is_file() {
        files.push((shown.to_string(), at.to_path_buf()));
        return Ok(());
    }
    if !at.is_dir() {
        return Err(format!("{shown}: no such patch or directory of patches"));
    }
    let listed = at.join("series");
    if listed.is_file() {
        let text = std::fs::read_to_string(&listed).map_err(|e| format!("{shown}/series: {e}"))?;
        for line in text.lines() {
            // quilt's form: a patch per line, then its options; `#` begins
            // a comment.
            let line = line.split('#').next().unwrap_or_default().trim();
            let Some(name) = line.split_whitespace().next() else {
                continue;
            };
            if let Some(option) = line.split_whitespace().nth(1)
                && option != "-p1"
            {
                return Err(format!(
                    "{shown}/series: {name} is applied with {option}; every patch here is -p1"
                ));
            }
            let relative = inside(name)?;
            let file = at.join(&relative);
            if !file.is_file() {
                return Err(format!("{shown}/series names {name}, which is not there"));
            }
            files.push((format!("{shown}/{relative}"), file));
        }
        return Ok(());
    }
    let mut names: Vec<String> = std::fs::read_dir(at)
        .map_err(|e| format!("{shown}: {e}"))?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.ends_with(".patch"))
        .collect();
    names.sort();
    for name in names {
        files.push((format!("{shown}/{name}"), at.join(&name)));
    }
    Ok(())
}

/// `path` as a relative path with no `..`, or why not.
fn inside(path: &str) -> Result<String, String> {
    let parts: Vec<&str> = path
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    if path.starts_with('/') || parts.is_empty() || parts.contains(&"..") {
        return Err(format!("`{path}` is not a path inside the tree"));
    }
    Ok(parts.join("/"))
}
