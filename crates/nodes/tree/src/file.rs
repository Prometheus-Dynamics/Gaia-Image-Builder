//! `gaia.tree.file`: a tree with one image added as a file.

use crate::walk::{Node, copy, inside, list, make_dirs, set_mode};
use crate::{entries_text, tree_input};
use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{FileEntry, Image, Tree};
use std::path::Path;

const NAME: &str = "gaia.tree.file";
/// The added file's mode: it is data, read by root.
const MODE: u32 = 0o644;

/// `tree` with `file` at `path` (owned by root, mode 0644), or a tree with
/// only that file when there is no `tree`. A path the tree already has is
/// an error: nothing is replaced.
///
/// For an image that lives as a file in another filesystem: a slot on a
/// data partition.
#[node(id = "gaia.tree.file", inputs(store, tree, file, path), outputs(tree))]
pub fn tree_file(
    store: &str,
    tree: Option<&Tree>,
    file: &Image,
    path: &str,
    cx: &Ctx,
) -> Result<Tree, StepError> {
    let fail = |why: String| StepError::body(NAME, why);
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let to = inside(path).map_err(fail)?;
    let source = file
        .object
        .object_ref()
        .ok_or_else(|| fail("the file is not in the store".into()))?;
    let source_path = store.path(&source);
    if !source_path.is_file() {
        return Err(fail(format!(
            "the file for {path} is not a file in the store"
        )));
    }
    let mut key = KeyBuilder::new(NAME, 1)
        .config("path", &to)
        .input("file", source.digest);
    let base = match tree {
        Some(tree) => {
            key = key
                .config("entries", entries_text(tree))
                .input("tree", tree_input(tree, NAME)?);
            tree.object.path_in(&store)
        }
        None => None,
    };
    let made = Step::new(&store, cx, NAME, key.finish()).look_up_or_make(
        &[("tree", Path::new("tree"))],
        |work| {
            let io = |e: std::io::Error| StepError::body(NAME, e.to_string());
            let out = work.dir().join("tree");
            make_dirs(&out).map_err(io)?;
            if let Some(base) = &base {
                let found = list(base).map_err(io)?;
                if let Some(taken) = found.get(&to) {
                    return Err(StepError::body(
                        NAME,
                        format!("/{to} is already {} in the tree", taken.node.name()),
                    ));
                }
                for (name, at) in &found {
                    let dst = out.join(name);
                    match at.node {
                        Node::Dir => std::fs::create_dir(&dst).map_err(io)?,
                        _ => copy(&base.join(name), &dst).map_err(io)?,
                    }
                }
                for (name, at) in found.iter().rev() {
                    if !matches!(at.node, Node::Link(_)) {
                        set_mode(&out.join(name), at.mode).map_err(io)?;
                    }
                }
            }
            let dst = out.join(&to);
            let parent = dst.parent().unwrap_or(&out);
            if parent.is_file() {
                return Err(StepError::body(
                    NAME,
                    format!("/{to}: its directory is a file in the tree"),
                ));
            }
            make_dirs(parent).map_err(io)?;
            std::fs::copy(&source_path, &dst).map_err(io)?;
            set_mode(&dst, MODE).map_err(io)
        },
    )?;
    let mut entries = tree.map(|t| t.entries.clone()).unwrap_or_default();
    entries.push(FileEntry {
        path: to,
        mode: MODE,
        uid: 0,
        gid: 0,
    });
    entries.sort();
    Ok(Tree {
        object: made.outputs["tree"].into(),
        entries,
    })
}
