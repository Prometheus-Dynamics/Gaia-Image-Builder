//! `gaia.tree.select`: declared files of one tree added to another.
//!
//! For a boot directory made from a build's outputs: the device trees and
//! overlays a board's firmware reads, out of everything its kernel built,
//! each at the path the firmware looks for it.

use crate::walk::{Node, copy, inside, list, make_dirs, set_mode};
use crate::{entries_text, tree_input};
use daedalus::prelude::*;
use gaia_step::{Ctx, Digest, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, FileEntry, Tree};
use std::collections::BTreeMap;
use std::path::Path;

const NAME: &str = "gaia.tree.select";
/// A selected file's mode: data, read by root.
const MODE: u32 = 0o644;

/// `tree` with the files `files` names from `from` (a tree: a build's
/// directory of outputs), or a tree of only those. `files` is a whitespace-separated list of words: `path` puts the
/// file at `path` in `from` at the same path, `path=to` puts it at `to`. A
/// file `from` does not have, a path the tree already has, a path named
/// twice and an empty list are errors: nothing is replaced or guessed.
#[node(
    id = "gaia.tree.select",
    inputs(store, tree, from, files),
    outputs(tree)
)]
pub fn tree_select(
    store: &str,
    tree: Option<&Tree>,
    from: &Artifact,
    files: &str,
    cx: &Ctx,
) -> Result<Tree, StepError> {
    let fail = |why: String| StepError::body(NAME, why);
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let picks = parse(files).map_err(fail)?;
    let object = from
        .object
        .object_ref()
        .ok_or_else(|| fail("the tree to select from is not in the store".into()))?;
    let stored = store.path(&object);
    if !stored.is_dir() {
        return Err(fail(
            "the artifact to select from is one file, not a tree".into(),
        ));
    }
    let mut key = KeyBuilder::new(NAME, 1);
    let mut sources = Vec::new();
    for (src, to) in &picks {
        let file = stored.join(src);
        if !file.is_file() {
            return Err(fail(format!("the tree to select from has no file {src}")));
        }
        let bytes = std::fs::read(&file).map_err(|e| fail(format!("{src}: {e}")))?;
        key = key.input(&format!("file {to}"), Digest::of_bytes(&bytes));
        sources.push((to.clone(), bytes));
    }
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
                for (to, _) in &sources {
                    if let Some(taken) = found.get(to) {
                        return Err(StepError::body(
                            NAME,
                            format!("/{to} is already {} in the tree", taken.node.name()),
                        ));
                    }
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
            for (to, bytes) in &sources {
                let dst = out.join(to);
                let parent = dst.parent().unwrap_or(&out);
                if parent.is_file() {
                    return Err(StepError::body(
                        NAME,
                        format!("/{to}: its directory is a file in the tree"),
                    ));
                }
                make_dirs(parent).map_err(io)?;
                std::fs::write(&dst, bytes).map_err(io)?;
                set_mode(&dst, MODE).map_err(io)?;
            }
            Ok(())
        },
    )?;
    let mut entries = tree.map(|t| t.entries.clone()).unwrap_or_default();
    entries.extend(picks.into_iter().map(|(_, path)| FileEntry {
        path,
        mode: MODE,
        uid: 0,
        gid: 0,
    }));
    entries.sort();
    Ok(Tree {
        object: made.outputs["tree"].into(),
        entries,
    })
}

/// The `(source, destination)` pairs `files` names, in its order.
fn parse(files: &str) -> Result<Vec<(String, String)>, String> {
    let mut picks = Vec::new();
    let mut seen = BTreeMap::new();
    for word in files.split_whitespace() {
        let (src, to) = word.split_once('=').unwrap_or((word, word));
        let src = inside(src)?;
        let to = inside(to)?;
        if let Some(first) = seen.insert(to.clone(), word.to_string()) {
            return Err(format!("/{to} is named twice: `{first}` and `{word}`"));
        }
        picks.push((src, to));
    }
    if picks.is_empty() {
        return Err("no files to select".into());
    }
    Ok(picks)
}
