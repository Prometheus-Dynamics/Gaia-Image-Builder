//! `gaia.tree.artifact`: a tree as an artifact of a kind, the same stored object.
//!
//! A file the board names (`file = "assets/u-boot-env.bin"`, written as it is) comes
//! from a file input, which is a tree of that one file; a consumer that takes an
//! artifact (a disk blob) takes this. No bytes are made.

use daedalus::prelude::*;
use gaia_types::{Artifact, ArtifactKind, Tree};

/// The node: `tree` as an artifact of `kind`.
#[node(id = "gaia.tree.artifact", inputs(tree, kind), outputs(artifact))]
pub fn tree_artifact(tree: &Tree, kind: &str) -> Artifact {
    Artifact {
        kind: ArtifactKind::parse(kind),
        object: tree.object.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_types::{ObjectKind, StoreRef};

    #[test]
    fn the_artifact_is_the_same_stored_tree_with_the_kind_given() {
        let tree = Tree {
            object: StoreRef {
                kind: ObjectKind::Tree,
                digest: "abc".into(),
            },
            entries: Vec::new(),
        };
        let artifact = tree_artifact(&tree, "uboot-env");
        assert_eq!(artifact.kind.as_str(), "uboot-env");
        assert_eq!(artifact.object, tree.object);
    }
}
