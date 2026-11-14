//! `gaia.tree.links`: symbolic links as one owner's layer.

use crate::walk::inside;
use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Layer, Tree};
use std::path::Path;

const NAME: &str = "gaia.tree.links";

/// A layer of symbolic links owned by `owner`. `links` is a JSON array of
/// `[path, target]` pairs (text, since a graph sets it as one constant):
/// each `path` in the image is a link to `target`, written as given. The
/// target need not be in the image: a path that is made when the system
/// runs (`/run/...`) is what such a link is for.
///
/// A path listed twice, or one that is another's parent, is an error.
#[node(id = "gaia.tree.links", inputs(store, owner, links), outputs(layer))]
pub fn tree_links(store: &str, owner: &str, links: &str, cx: &Ctx) -> Result<Layer, StepError> {
    let fail = |why: String| StepError::body(NAME, why);
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    if owner.is_empty() {
        return Err(fail("a layer needs an owner".into()));
    }
    let pairs: Vec<(String, String)> = serde_json::from_str(links).map_err(|e| {
        fail(format!(
            "the links are not a list of [path, target] pairs: {e}"
        ))
    })?;
    let mut wanted: Vec<(String, String)> = Vec::new();
    for (path, target) in pairs {
        let path = inside(&path).map_err(fail)?;
        if target.is_empty() || target.contains('\0') {
            return Err(fail(format!("/{path} is a link to nothing")));
        }
        if let Some((other, _)) = wanted.iter().find(|(other, _)| {
            *other == path
                || other.starts_with(&format!("{path}/"))
                || path.starts_with(&format!("{other}/"))
        }) {
            return Err(fail(format!(
                "/{path} and /{other} cannot both be links: one is the other, or its parent"
            )));
        }
        wanted.push((path, target));
    }
    wanted.sort();
    let text: Vec<String> = wanted
        .iter()
        .map(|(path, target)| format!("{path}\t{target}"))
        .collect();
    let key = KeyBuilder::new(NAME, 1)
        .config("links", text.join("\n"))
        .finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("tree", Path::new("tree"))],
        |work| {
            let io = |e: std::io::Error| StepError::body(NAME, e.to_string());
            let out = work.dir().join("tree");
            std::fs::create_dir(&out).map_err(io)?;
            for (path, target) in &wanted {
                let at = out.join(path);
                if let Some(parent) = at.parent() {
                    std::fs::create_dir_all(parent).map_err(io)?;
                }
                std::os::unix::fs::symlink(target, &at).map_err(io)?;
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
