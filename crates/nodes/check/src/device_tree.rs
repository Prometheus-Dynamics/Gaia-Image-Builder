//! `gaia.check.device-tree`: the device tree nodes a board names are in the
//! device tree blobs it boots.
//!
//! Each blob is a file in the tree (the board's `.dtb`, or an overlay built
//! for it). A node is named by its path from the root, unit addresses
//! included (`/soc/i2c@7e804000`), and must be there exactly.

use crate::common::{parse_requirements, root, verdict};
use crate::fdt;
use crate::tree;
use daedalus::prelude::*;
use gaia_step::{Ctx, StepError};
use gaia_types::Tree;
use serde::Deserialize;
use std::path::Path;

const NAME: &str = "gaia.check.device-tree";

/// The blobs to check, and for each the node paths that must exist in it.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Requirements {
    pub blobs: Vec<Blob>,
}

/// One device tree blob in the tree and the nodes it must hold.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Blob {
    pub file: String,
    pub nodes: Vec<String>,
}

/// Fails when a named node is not in its blob. Gives the tree unchanged.
#[node(
    id = "gaia.check.device-tree",
    inputs(store, tree, requirements),
    outputs(tree)
)]
pub fn check_device_tree(
    store: &str,
    tree: Tree,
    requirements: &str,
    _cx: &Ctx,
) -> Result<Tree, StepError> {
    let wanted: Requirements = parse_requirements(NAME, requirements)?;
    let dir = root(NAME, store, &tree)?;
    verdict(NAME, problems(&dir, &wanted))?;
    Ok(tree)
}

/// Every named node that is missing, or every blob that is not one, as one
/// line each.
pub fn problems(root: &Path, wanted: &Requirements) -> Vec<String> {
    let mut problems = Vec::new();
    for blob in &wanted.blobs {
        let Some(bytes) = tree::read(root, &blob.file) else {
            problems.push(format!("device tree {} is not in the image", blob.file));
            continue;
        };
        let parsed = match fdt::parse(&bytes) {
            Ok(parsed) => parsed,
            Err(why) => {
                problems.push(format!("{} is not a device tree blob: {why}", blob.file));
                continue;
            }
        };
        for node in &blob.nodes {
            if !parsed.has(node) {
                problems.push(format!("device tree {} has no node {node}", blob.file));
            }
        }
    }
    problems
}
