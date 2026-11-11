//! Stored things: objects, artifacts, trees and their file entries.

use crate::kind::open_kind;
use daedalus::prelude::Type;
use gaia_store::{Digest, Kind, ObjectRef, Store};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Whether a stored object is a file or a tree.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Type, Serialize, Deserialize,
)]
#[type_key("gaia:object-kind")]
#[serde(rename_all = "lowercase")]
pub enum ObjectKind {
    File,
    Tree,
}

/// A file or tree in the content store, by digest (`sha256:<hex>`). Where
/// it sits in a store follows from it ([`StoreRef::path_in`]), so no host
/// path crosses a port.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Type, Serialize, Deserialize)]
#[type_key("gaia:object")]
pub struct StoreRef {
    pub kind: ObjectKind,
    pub digest: String,
}

impl StoreRef {
    /// The store's reference, when the digest is well formed.
    pub fn object_ref(&self) -> Option<ObjectRef> {
        Some(ObjectRef {
            kind: match self.kind {
                ObjectKind::File => Kind::File,
                ObjectKind::Tree => Kind::Tree,
            },
            digest: Digest::parse(&self.digest)?,
        })
    }

    /// Where the object is in `store`.
    pub fn path_in(&self, store: &Store) -> Option<PathBuf> {
        self.object_ref().map(|object| store.path(&object))
    }
}

impl From<ObjectRef> for StoreRef {
    fn from(object: ObjectRef) -> Self {
        Self {
            kind: match object.kind {
                Kind::File => ObjectKind::File,
                Kind::Tree => ObjectKind::Tree,
            },
            digest: object.digest.to_string(),
        }
    }
}

open_kind!(
    /// What an artifact is.
    ArtifactKind, "gaia:artifact-kind" {
        Binary => "binary",
        Library => "library",
        Archive => "archive",
        Package => "package",
        Kernel => "kernel",
        DeviceTree => "device-tree",
        Firmware => "firmware",
        Config => "config",
        Tree => "tree",
    }
);

/// Something a step made: what it is and where it is stored.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:artifact")]
pub struct Artifact {
    pub kind: ArtifactKind,
    pub object: StoreRef,
}

/// One path's mode and owner. The store keeps modes but never owners, so
/// what an image needs (root-owned files, setuid bits) is said here.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Type, Serialize, Deserialize)]
#[type_key("gaia:file-entry")]
pub struct FileEntry {
    /// Relative to the tree's root, `/`-separated.
    pub path: String,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

/// A stored tree, with the owners and modes its paths get in an image.
/// Paths not listed keep their stored mode and belong to root.
#[derive(Clone, Debug, PartialEq, Eq, Type, Serialize, Deserialize)]
#[type_key("gaia:tree")]
pub struct Tree {
    pub object: StoreRef,
    pub entries: Vec<FileEntry>,
}
