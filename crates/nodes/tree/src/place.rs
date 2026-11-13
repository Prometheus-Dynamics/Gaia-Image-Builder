//! `gaia.tree.place`: install entries placed with their modes and owners.

use crate::walk::{copy, inside, make_dirs, set_mode};
use crate::{entries_text, tree_input};
use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{FileEntry, Layer, Placements, Tree};
use std::collections::BTreeSet;
use std::path::Path;

const NAME: &str = "gaia.tree.place";

/// `owner`'s layer: each placement's `from` in `from`, at its `to`.
#[node(
    id = "gaia.tree.place",
    inputs(store, owner, from, placements),
    outputs(layer)
)]
pub fn tree_place(
    store: &str,
    owner: &str,
    from: Tree,
    placements: &Placements,
    cx: &Ctx,
) -> Result<Layer, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let src = from
        .object
        .path_in(&store)
        .ok_or_else(|| StepError::body(NAME, "the source tree is not in the store"))?;
    let mut targets = BTreeSet::new();
    let mut checked = Vec::new();
    for placement in &placements.entries {
        let from_path = inside(&placement.from).map_err(|e| StepError::body(NAME, e))?;
        let to_path = inside(&placement.to).map_err(|e| StepError::body(NAME, e))?;
        if !targets.insert(to_path.clone()) {
            return Err(StepError::body(
                NAME,
                format!("{owner}: two entries put a file at {}", placement.to),
            ));
        }
        if std::fs::symlink_metadata(src.join(&from_path)).is_err() {
            return Err(StepError::body(
                NAME,
                format!("{owner}: {} is not in its source", placement.from),
            ));
        }
        checked.push((from_path, to_path, placement));
    }
    let mut key = KeyBuilder::new(NAME, 1)
        .config("owner", owner)
        .config("source entries", entries_text(&from))
        .input("source", tree_input(&from, NAME)?);
    for (i, (from_path, to_path, p)) in checked.iter().enumerate() {
        key = key.config(
            &format!("placement {i}"),
            format!("{from_path} {to_path} {:?} {} {}", p.mode, p.uid, p.gid),
        );
    }
    let made = Step::new(&store, cx, NAME, key.finish()).look_up_or_make(
        &[("tree", Path::new("tree"))],
        |work| {
            let out = work.dir().join("tree");
            make_dirs(&out).map_err(|e| StepError::body(NAME, e))?;
            for (from_path, to_path, p) in &checked {
                let dst = out.join(to_path);
                let parent = dst.parent().unwrap_or(&out);
                make_dirs(parent).map_err(|e| StepError::body(NAME, e))?;
                copy(&src.join(from_path), &dst)
                    .map_err(|e| StepError::body(NAME, format!("{}: {e}", p.from)))?;
                if let Some(mode) = p.mode {
                    set_mode(&dst, mode).map_err(|e| StepError::body(NAME, e))?;
                }
            }
            Ok(())
        },
    )?;
    let object = made.outputs["tree"];
    let placed = store.path(&object);
    let mut entries = Vec::new();
    for (_, to_path, p) in &checked {
        let mode = match p.mode {
            Some(mode) => mode & 0o7777,
            None => stored_mode(&placed.join(to_path)).map_err(|e| StepError::body(NAME, e))?,
        };
        entries.push(FileEntry {
            path: to_path.clone(),
            mode,
            uid: p.uid,
            gid: p.gid,
        });
    }
    entries.sort();
    Ok(Layer {
        owner: owner.to_string(),
        tree: Tree {
            object: object.into(),
            entries,
        },
        replaces: Vec::new(),
    })
}

fn stored_mode(path: &Path) -> std::io::Result<u32> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::symlink_metadata(path)?;
    Ok(if meta.file_type().is_symlink() {
        0
    } else {
        meta.permissions().mode() & 0o7777
    })
}
