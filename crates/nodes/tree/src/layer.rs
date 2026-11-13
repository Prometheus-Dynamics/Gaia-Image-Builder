//! `gaia.tree.layer`: a whole tree as one owner's layer.

use daedalus::prelude::*;
use gaia_step::StepError;
use gaia_types::{Layer, Tree};

/// `tree`, every path in it claimed by `owner`: an OS's root filesystem, or
/// a component's build output when it lists no install entries. Nothing is
/// copied, so it is not a step of its own and makes nothing in the store.
#[node(id = "gaia.tree.layer", inputs(owner, tree), outputs(layer))]
pub fn tree_layer(owner: &str, tree: Tree) -> Result<Layer, StepError> {
    if owner.is_empty() {
        return Err(StepError::body("gaia.tree.layer", "a layer needs an owner"));
    }
    Ok(Layer {
        owner: owner.to_string(),
        tree,
        replaces: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_types::{ObjectKind, StoreRef};

    #[test]
    fn the_layer_is_the_tree_under_its_owner() {
        let tree = Tree {
            object: StoreRef {
                kind: ObjectKind::Tree,
                digest: "sha256:00".into(),
            },
            entries: Vec::new(),
        };
        let layer = tree_layer("os", tree.clone()).expect("a layer");
        assert_eq!(layer.owner, "os");
        assert_eq!(layer.tree, tree);
        assert!(layer.replaces.is_empty());
    }

    #[test]
    fn a_layer_without_an_owner_is_an_error() {
        let tree = Tree {
            object: StoreRef {
                kind: ObjectKind::Tree,
                digest: "sha256:00".into(),
            },
            entries: Vec::new(),
        };
        assert!(tree_layer("", tree).is_err());
    }
}
