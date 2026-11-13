//! `gaia.tree.pick`: one file of a tree, as a tree of its own.

use crate::walk::inside;
use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::Tree;
use std::path::Path;

const NAME: &str = "gaia.tree.pick";

/// The file at `path` in `tree`, alone in a new tree at the same path.
///
/// For a step that reads one file of a large source: given the whole
/// source it would run again on any change to it; given this, only when
/// the file changes.
#[node(id = "gaia.tree.pick", inputs(store, tree, path), outputs(tree))]
pub fn tree_pick(store: &str, tree: Tree, path: &str, cx: &Ctx) -> Result<Tree, StepError> {
    let fail = |why: String| StepError::body(NAME, why);
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let relative = inside(path).map_err(fail)?;
    let object = tree
        .object
        .object_ref()
        .ok_or_else(|| fail("a tree that is not in the store".into()))?;
    let src = store.path(&object).join(&relative);
    if !src.is_file() {
        return Err(fail(format!("the tree has no file {relative}")));
    }
    let key = KeyBuilder::new(NAME, 1)
        .config("path", &relative)
        .input("tree", object.digest)
        .finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("tree", Path::new("tree"))],
        |work| {
            let io = |e: std::io::Error| StepError::body(NAME, e.to_string());
            let to = work.dir().join("tree").join(&relative);
            std::fs::create_dir_all(to.parent().unwrap_or(work.dir())).map_err(io)?;
            std::fs::copy(&src, &to).map_err(io)?;
            Ok(())
        },
    )?;
    Ok(Tree {
        object: made.outputs["tree"].into(),
        entries: Vec::new(),
    })
}
