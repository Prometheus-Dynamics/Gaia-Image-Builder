//! `gaia.tree.under`: an artifact's files as a layer under a directory.

use crate::walk::{Node, copy, inside, list, make_dirs, set_mode};
use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, Layer, ObjectKind, Tree};
use std::path::Path;

const NAME: &str = "gaia.tree.under";

/// Everything `from` holds, at `prefix` (a directory in the image): the
/// tree's own paths under it, or the one file by `name`. `owner` claims the
/// paths. An empty tree gives an empty layer: nothing, not even `prefix`.
///
/// For what one step makes and another tree carries: a kernel's modules
/// under `/lib/modules`, its image and device trees under `/boot`.
#[node(
    id = "gaia.tree.under",
    inputs(store, owner, from, prefix),
    outputs(layer)
)]
pub fn tree_under(
    store: &str,
    owner: &str,
    from: &Artifact,
    prefix: &str,
    cx: &Ctx,
) -> Result<Layer, StepError> {
    let fail = |why: String| StepError::body(NAME, why);
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    if owner.is_empty() {
        return Err(fail("a layer needs an owner".into()));
    }
    let prefix = inside(prefix).map_err(fail)?;
    if from.object.kind != ObjectKind::Tree {
        return Err(fail(format!(
            "{owner}: what goes under /{prefix} is a file, not a tree of files"
        )));
    }
    let object = from
        .object
        .object_ref()
        .ok_or_else(|| fail(format!("{owner}: its source is not in the store")))?;
    let src = store.path(&object);
    let key = KeyBuilder::new(NAME, 1)
        .config("prefix", &prefix)
        .input("from", object.digest)
        .finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("tree", Path::new("tree"))],
        |work| {
            let io = |e: std::io::Error| StepError::body(NAME, e.to_string());
            let out = work.dir().join("tree");
            make_dirs(&out).map_err(io)?;
            let found = list(&src).map_err(io)?;
            // The listing has the tree's own root as the empty path.
            let entries: Vec<_> = found.iter().filter(|(path, _)| !path.is_empty()).collect();
            if entries.is_empty() {
                return Ok(());
            }
            let base = out.join(&prefix);
            make_dirs(&base).map_err(io)?;
            for (path, at) in &entries {
                let dst = base.join(path);
                match at.node {
                    Node::Dir => std::fs::create_dir(&dst).map_err(io)?,
                    _ => copy(&src.join(path), &dst).map_err(io)?,
                }
            }
            for (path, at) in entries.iter().rev() {
                if !matches!(at.node, Node::Link(_)) {
                    set_mode(&base.join(path), at.mode).map_err(io)?;
                }
            }
            Ok(())
        },
    )?;
    Ok(Layer {
        owner: owner.to_string(),
        tree: Tree {
            object: made.outputs["tree"].into(),
            entries: Vec::new(),
        },
        replaces: Vec::new(),
    })
}
