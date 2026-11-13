//! `gaia.tree.merge`: layers into one tree, every path owned.

use crate::walk::{Found, Node, copy, inside, list, make_dirs, set_mode};
use crate::{entries_text, tree_input};
use daedalus::prelude::*;
use daedalus_cpu_author::FanIn;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Artifact, ArtifactKind, FileEntry, Layer, Owned, Tree};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.tree.merge";

/// Who has a path in the merged tree, and where it comes from.
struct Claim {
    node: Node,
    mode: u32,
    uid: u32,
    gid: u32,
    owners: Vec<String>,
    /// The layer's stored tree the path is copied from.
    from: PathBuf,
    /// The layer that replaced it, once one has.
    replaced_by: Option<String>,
    /// For a directory: whether a layer chose its mode and owner (an entry
    /// for it, or a stored mode that is not the plain 0755). A directory a
    /// layer has only because a file is under it is not chosen.
    chosen: bool,
}

/// The mode a directory has when nothing chose one.
const PLAIN_DIR: u32 = 0o755;

/// The node: any number of layers, on ports `layers0`, `layers1` and so
/// on, merged in port order.
#[node(id = "gaia.tree.merge", inputs(store, layers), outputs(tree, owners))]
pub fn tree_merge(
    store: &str,
    layers: FanIn<'_, Layer>,
    cx: &Ctx,
) -> Result<(Tree, Artifact), StepError> {
    let layers: Vec<Layer> = layers.iter().map(|layer| (*layer).clone()).collect();
    merge_layers(store, &layers, cx)
}

/// The merged tree, and `owners.json`: every path with its mode, owner ids
/// and the layers that claim it.
pub fn merge_layers(
    store: &str,
    layers: &[Layer],
    cx: &Ctx,
) -> Result<(Tree, Artifact), StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let mut key = KeyBuilder::new(NAME, 1);
    for (i, layer) in layers.iter().enumerate() {
        key = key
            .config(&format!("layer {i} owner"), &layer.owner)
            .config(&format!("layer {i} entries"), entries_text(&layer.tree))
            .config(&format!("layer {i} replaces"), layer.replaces.join("\n"))
            .input(&format!("layer {i}"), tree_input(&layer.tree, NAME)?);
    }
    let made = Step::new(&store, cx, NAME, key.finish()).look_up_or_make(
        &[
            ("tree", Path::new("tree")),
            ("owners", Path::new("owners.json")),
        ],
        |work| {
            let claims = claim(&store, layers)?;
            let out = work.dir().join("tree");
            write(&out, &claims).map_err(|e| StepError::body(NAME, e))?;
            let owned: Vec<Owned> = claims
                .iter()
                .map(|(path, c)| Owned {
                    path: path.clone(),
                    mode: c.mode,
                    uid: c.uid,
                    gid: c.gid,
                    owners: c.owners.clone(),
                })
                .collect();
            let text = serde_json::to_vec_pretty(&owned).map_err(|e| StepError::body(NAME, e))?;
            std::fs::write(work.dir().join("owners.json"), text)
                .map_err(|e| StepError::body(NAME, e))
        },
    )?;
    let owners_ref = made.outputs["owners"];
    let owned: Vec<Owned> = serde_json::from_slice(
        &std::fs::read(store.path(&owners_ref)).map_err(|e| StepError::body(NAME, e))?,
    )
    .map_err(|e| StepError::body(NAME, e))?;
    let entries = owned
        .into_iter()
        .map(|o| FileEntry {
            path: o.path,
            mode: o.mode,
            uid: o.uid,
            gid: o.gid,
        })
        .collect();
    Ok((
        Tree {
            object: made.outputs["tree"].into(),
            entries,
        },
        Artifact {
            kind: ArtifactKind::parse("owners"),
            object: owners_ref.into(),
        },
    ))
}

/// Every path of every layer, claimed in order.
fn claim(store: &Store, layers: &[Layer]) -> Result<BTreeMap<String, Claim>, StepError> {
    let mut claims: BTreeMap<String, Claim> = BTreeMap::new();
    for layer in layers {
        let owner = &layer.owner;
        let root = layer.tree.object.path_in(store).ok_or_else(|| {
            StepError::body(NAME, format!("{owner}: its tree is not in the store"))
        })?;
        let found = list(&root).map_err(|e| StepError::body(NAME, format!("{owner}: {e}")))?;
        let mut set: BTreeMap<&str, &gaia_types::FileEntry> = BTreeMap::new();
        for entry in &layer.tree.entries {
            if !found.contains_key(&entry.path) {
                return Err(StepError::body(
                    NAME,
                    format!(
                        "{owner}: an owner is set for {}, which it does not have",
                        entry.path
                    ),
                ));
            }
            set.insert(&entry.path, entry);
        }
        let replaces: BTreeSet<String> = layer
            .replaces
            .iter()
            .map(|path| inside(path))
            .collect::<Result<_, _>>()
            .map_err(|e| StepError::body(NAME, format!("{owner}: {e}")))?;
        for path in &replaces {
            if !found.contains_key(path) {
                return Err(StepError::body(
                    NAME,
                    format!("{owner}: it replaces {path}, which it does not have"),
                ));
            }
        }
        for (path, Found { node, mode }) in found {
            let chosen = set.contains_key(path.as_str()) || mode != PLAIN_DIR;
            let (mode, uid, gid) = match set.get(path.as_str()) {
                Some(entry) => (entry.mode & 0o7777, entry.uid, entry.gid),
                None => (mode, 0, 0),
            };
            let new = Claim {
                chosen,
                node,
                mode,
                uid,
                gid,
                owners: vec![owner.clone()],
                from: root.join(&path),
                replaced_by: None,
            };
            let Some(old) = claims.get_mut(&path) else {
                claims.insert(path, new);
                continue;
            };
            let same = (old.mode, old.uid, old.gid) == (new.mode, new.uid, new.gid);
            if old.node == Node::Dir && new.node == Node::Dir && same {
                old.owners.push(owner.clone());
                old.chosen |= new.chosen;
                continue;
            }
            // A directory one layer chose a mode and owner for, and another
            // has only as the parent of its files: the choice stands. Two
            // different choices are still a conflict.
            if old.node == Node::Dir && new.node == Node::Dir && old.chosen != new.chosen {
                let mut owners = std::mem::take(&mut old.owners);
                owners.push(owner.clone());
                if new.chosen {
                    *old = Claim { owners, ..new };
                } else {
                    old.owners = owners;
                }
                continue;
            }
            if replaces.contains(&path) {
                if let Some(first) = &old.replaced_by {
                    return Err(StepError::body(
                        NAME,
                        format!("/{path}: replaced by both {first} and {owner}"),
                    ));
                }
                *old = Claim {
                    replaced_by: Some(owner.clone()),
                    ..new
                };
                continue;
            }
            let theirs = old.owners.join(", ");
            let why = if old.node != new.node {
                format!(
                    "{} in {theirs}, {} in {owner}",
                    old.node.name(),
                    new.node.name()
                )
            } else if old.node == Node::Dir {
                format!(
                    "{:o} {}:{} in {theirs}, {:o} {}:{} in {owner}",
                    old.mode, old.uid, old.gid, new.mode, new.uid, new.gid
                )
            } else {
                format!("claimed by both {theirs} and {owner}")
            };
            return Err(StepError::body(NAME, format!("/{path}: {why}")));
        }
    }
    // A path under something that is not a directory, from another layer.
    for path in claims.keys() {
        if let Some((parent, _)) = path.rsplit_once('/')
            && claims.get(parent).is_some_and(|c| c.node != Node::Dir)
        {
            let (a, b) = (&claims[parent], &claims[path]);
            return Err(StepError::body(
                NAME,
                format!(
                    "/{path}: in {}, under /{parent}, {} in {}",
                    b.owners.join(", "),
                    a.node.name(),
                    a.owners.join(", ")
                ),
            ));
        }
    }
    Ok(claims)
}

/// Writes the claimed paths, parents first, then sets each mode (deepest
/// first, so a read-only directory is set after what it holds).
fn write(out: &Path, claims: &BTreeMap<String, Claim>) -> std::io::Result<()> {
    make_dirs(out)?;
    for (path, claim) in claims {
        let dst = out.join(path);
        match &claim.node {
            Node::Dir => std::fs::create_dir(&dst)?,
            Node::File => {
                std::fs::copy(&claim.from, &dst)?;
            }
            Node::Link(_) => copy(&claim.from, &dst)?,
        }
    }
    for (path, claim) in claims.iter().rev() {
        if !matches!(claim.node, Node::Link(_)) {
            set_mode(&out.join(path), claim.mode)?;
        }
    }
    Ok(())
}
