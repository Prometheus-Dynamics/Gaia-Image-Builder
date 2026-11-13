//! File trees for an image: each component's files placed with their modes
//! and owners, the files generated from descriptions, and all of it merged
//! with an owner named for every path.
//!
//! - **`gaia.tree.place`**: a component's install entries, taken from a
//!   tree (its build output or its files) and placed at their paths in the
//!   image with their modes and owners. Gives the component's
//!   [`Layer`](gaia_types::Layer).
//! - **`gaia.tree.accounts`**: the users and groups components need, added
//!   to the base's `/etc/passwd`, `/etc/group` and `/etc/shadow`. Ids not
//!   given are the lowest free from 1000, in name order. Gives a layer that
//!   replaces those three files.
//! - **`gaia.tree.services`**: a component's enabled units as the links
//!   `systemctl enable` makes, read from each unit's `[Install]` section.
//! - **`gaia.tree.merge`**: layers into one tree, in order. The node takes
//!   them as a fan-in (`layers0`, `layers1`, ...), one edge per layer;
//!   [`merge_layers`] takes a slice. Two layers
//!   claiming one path is an error naming both, unless the later one says it
//!   replaces that path (and only one may). A directory may be shared when
//!   both give it the same mode and owner. Every path of the result has an
//!   entry: its mode, its owner ids, and the layers that own it.
//!
//! The store keeps modes but never owners, so a tree's owners are in
//! [`Tree::entries`]. [`owned_digest`](gaia_types::owned_digest) is the digest of a tree with its
//! entries: what a later step keys on so an owner change reaches it.

mod accounts;
mod by_role;
mod file;
mod generate;
mod layer;
mod merge;
mod pick;
mod place;
mod services;
mod under;
mod walk;

pub use accounts::tree_accounts;
pub use by_role::{placements, tree_by_role};
pub use file::tree_file;
pub use generate::{TOKENS, generate, unknown_tokens};
pub use layer::tree_layer;
pub use merge::{merge_layers, tree_merge};
pub use pick::tree_pick;
pub use place::tree_place;
pub use services::tree_services;
pub use under::tree_under;

use daedalus::prelude::*;
use gaia_store::Digest;
use gaia_types::Tree;

/// The nodes, for a registry.
pub const NODES: [&NodeRecord; 10] = [
    &by_role::TREE_BY_ROLE_NODE,
    &generate::GENERATE_NODE,
    &pick::TREE_PICK_NODE,
    &under::TREE_UNDER_NODE,
    &place::TREE_PLACE_NODE,
    &file::TREE_FILE_NODE,
    &layer::TREE_LAYER_NODE,
    &accounts::TREE_ACCOUNTS_NODE,
    &services::TREE_SERVICES_NODE,
    &merge::TREE_MERGE_NODE,
];

/// A tree's digest, for a key.
pub(crate) fn tree_input(tree: &Tree, name: &str) -> Result<Digest, gaia_step::StepError> {
    tree.object
        .object_ref()
        .map(|object| object.digest)
        .ok_or_else(|| gaia_step::StepError::body(name, "a tree that is not in the store"))
}

/// A tree's entries, for a key.
pub(crate) fn entries_text(tree: &Tree) -> String {
    let mut entries = tree.entries.clone();
    entries.sort();
    entries
        .iter()
        .map(|e| format!("{} {:o} {} {}", e.path, e.mode, e.uid, e.gid))
        .collect::<Vec<_>>()
        .join("\n")
}
