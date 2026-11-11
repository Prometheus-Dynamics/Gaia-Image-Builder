//! `gaia.source.path`: a local directory as a stored tree.

use crate::files::copy_tree;
use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_store::tree_digest;
use gaia_types::Tree;
use std::path::Path;

const NAME: &str = "gaia.source.path";

/// Stores the directory at `path`, keyed on its tree digest.
#[node(id = "gaia.source.path", inputs(store, path), outputs(tree))]
pub fn source_path(store: &str, path: &str, cx: &Ctx) -> Result<Tree, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let dir = Path::new(path);
    if !dir.is_dir() {
        return Err(StepError::body(NAME, format!("{path} is not a directory")));
    }
    let digest = tree_digest(dir).map_err(|e| StepError::body(NAME, format!("{path}: {e}")))?;
    let key = KeyBuilder::new(NAME, 1).input("tree", digest).finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("tree", Path::new("tree"))],
        |work| {
            let copy = work.dir().join("tree");
            copy_tree(dir, &copy).map_err(|e| StepError::body(NAME, format!("{path}: {e}")))?;
            // Changed while it was copied: not the tree the key names.
            let copied = tree_digest(&copy).map_err(|e| StepError::body(NAME, e))?;
            if copied != digest {
                return Err(StepError::body(
                    NAME,
                    format!("{path} changed while it was copied: {digest} became {copied}"),
                ));
            }
            Ok(())
        },
    )?;
    Ok(Tree {
        object: made.outputs["tree"].into(),
        entries: Vec::new(),
    })
}
