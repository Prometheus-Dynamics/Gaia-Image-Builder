//! What the tree nodes pass between each other: layers, their install
//! entries, the accounts they need, and the owned digest of a tree.

use crate::Tree;
use daedalus::prelude::*;
use gaia_store::Digest;
use serde::{Deserialize, Serialize};

/// One owner's part of an image: a tree and who it belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:layer")]
pub struct Layer {
    /// Who claims its paths: a component's name, or the OS provider's.
    pub owner: String,
    pub tree: Tree,
    /// Paths an earlier layer has that this one replaces rather than
    /// conflicts with.
    pub replaces: Vec<String>,
}

/// One install entry: a path in the source tree, where it goes, and its
/// mode and owner ids.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:placement")]
pub struct Placement {
    pub from: String,
    /// An absolute path in the image.
    pub to: String,
    /// When not set, the stored mode.
    pub mode: Option<u32>,
    pub uid: u32,
    pub gid: u32,
}

/// A component's install entries from one tree, as one value: a host can
/// feed a keyed struct where a list of structs does not match the port.
#[derive(Clone, Debug, Default, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:placements")]
pub struct Placements {
    pub entries: Vec<Placement>,
}

/// A user a component needs.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:account")]
pub struct Account {
    /// The component that needs it.
    pub owner: String,
    pub name: String,
    pub uid: Option<u32>,
    /// Its primary group: one of its own name when not set.
    pub group: Option<String>,
    pub groups: Vec<String>,
    /// `/` when not set.
    pub home: Option<String>,
    /// `/bin/false` when not set.
    pub shell: Option<String>,
}

/// A group a component needs.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:account-group")]
pub struct AccountGroup {
    pub owner: String,
    pub name: String,
    pub gid: Option<u32>,
}

/// One path of a merged tree, as `owners.json` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owned {
    pub path: String,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    /// The layers that claim it: one, or several for a shared directory.
    pub owners: Vec<String>,
}

/// The digest of `tree` with its entries: changes when a file, a mode or
/// an owner does.
pub fn owned_digest(tree: &Tree) -> Option<Digest> {
    let object = tree.object.object_ref()?.digest;
    let mut entries = tree.entries.clone();
    entries.sort();
    let mut bytes = object.to_string().into_bytes();
    for entry in &entries {
        bytes.extend_from_slice(
            format!(
                "\n{} {:o} {} {}",
                entry.path, entry.mode, entry.uid, entry.gid
            )
            .as_bytes(),
        );
    }
    Some(Digest::of_bytes(&bytes))
}
