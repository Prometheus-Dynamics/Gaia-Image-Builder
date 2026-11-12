//! Copies of Gaia's port types, as conversion nodes from a type to itself.
//!
//! A value two nodes each take by value must be copied; the planner inserts
//! the type's copy where an output fans out to such consumers. The types
//! name stored objects, so a copy clones the reference, not the content.

use crate::{Artifact, Image, Tree};
use daedalus::prelude::*;

/// A copy of an artifact: the same stored object and kind.
#[node(id = "gaia.copy.artifact", conversion, inputs(value), outputs(copy))]
pub fn copy_artifact(value: &Artifact) -> Artifact {
    value.clone()
}

/// A copy of an image: the same stored object, size and partitions.
#[node(id = "gaia.copy.image", conversion, inputs(value), outputs(copy))]
pub fn copy_image(value: &Image) -> Image {
    value.clone()
}

/// A copy of a tree: the same stored tree and entries.
#[node(id = "gaia.copy.tree", conversion, inputs(value), outputs(copy))]
pub fn copy_tree(value: &Tree) -> Tree {
    value.clone()
}
