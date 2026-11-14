//! `gaia.tree.role`: a tree of files named by what they are.
//!
//! The program that reads a slot's boot directory owns its file names; a
//! build has each file by its *role* (`kernel`, `device-tree`, `cmdline`)
//! and [`crate::by_role`] places it where the reader's layout says. This
//! step makes the tree that step takes: it adds one file, under its role's
//! name, to the tree it is given (or starts one).
//!
//! The file comes from an artifact: the artifact itself when it is a file,
//! or the file at `path` in it when it is a tree. With no `path`, a tree
//! must hold exactly one file, which is taken: a kernel build's one image.
//! A role the tree already has is an error: nothing is replaced.

use crate::walk::{Node, copy, inside, list, make_dirs, set_mode};
use crate::{entries_text, tree_input};
use daedalus::prelude::*;
use gaia_step::{Ctx, Digest, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, FileEntry, Tree};
use std::path::Path;

const NAME: &str = "gaia.tree.role";
/// A role's file: data, read by root.
const MODE: u32 = 0o644;

/// `tree` with the file of `from` (at `path` in it, when it is a tree)
/// added under the name `role`; or a tree with only that file.
#[node(
    id = "gaia.tree.role",
    inputs(store, tree, from, path, role),
    outputs(tree)
)]
pub fn tree_role(
    store: &str,
    tree: Option<&Tree>,
    from: &Artifact,
    path: Option<&String>,
    role: &str,
    cx: &Ctx,
) -> Result<Tree, StepError> {
    let fail = |why: String| StepError::body(NAME, why);
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    if role.is_empty() || role == "." || role == ".." || role.contains('/') {
        return Err(fail(format!("`{role}` is not a role: one name")));
    }
    let object = from
        .object
        .object_ref()
        .ok_or_else(|| fail(format!("the {role} is not in the store")))?;
    let stored = store.path(&object);
    let path = path.map(String::as_str).unwrap_or_default();
    let source = if stored.is_file() {
        if !path.is_empty() {
            return Err(fail(format!(
                "the {role} is one file, and `{path}` names a path in it"
            )));
        }
        stored
    } else if path.is_empty() {
        let files: Vec<String> = list(&stored)
            .map_err(|e| fail(e.to_string()))?
            .into_iter()
            .filter(|(_, at)| matches!(at.node, Node::File))
            .map(|(name, _)| name)
            .collect();
        let [one] = files.as_slice() else {
            return Err(fail(format!(
                "the {role} is taken from a tree of {} files ({}); name the one",
                files.len(),
                files.join(", ")
            )));
        };
        stored.join(one)
    } else {
        let inner = inside(path).map_err(fail)?;
        let file = stored.join(&inner);
        if !file.is_file() {
            return Err(fail(format!("the {role}: no file {inner} was built")));
        }
        file
    };
    let bytes = std::fs::read(&source).map_err(|e| fail(format!("the {role}: {e}")))?;
    let mut key = KeyBuilder::new(NAME, 1)
        .config("role", role)
        .input("file", Digest::of_bytes(&bytes));
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
                if found.contains_key(role) {
                    return Err(StepError::body(
                        NAME,
                        format!("the tree already has a {role}"),
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
            let dst = out.join(role);
            std::fs::write(&dst, &bytes).map_err(io)?;
            set_mode(&dst, MODE).map_err(io)
        },
    )?;
    let mut entries = tree.map(|t| t.entries.clone()).unwrap_or_default();
    entries.push(FileEntry {
        path: role.to_string(),
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
