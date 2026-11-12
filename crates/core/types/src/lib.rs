//! The types that cross Gaia's node ports.
//!
//! Each is a structured Daedalus type (`#[derive(Type)]`) under a pinned key,
//! so the planner sees its fields and nothing at a port is opaque:
//!
//! | Key | Type |
//! |---|---|
//! | `gaia:object` | [`StoreRef`]: a file or tree in the content store |
//! | `gaia:artifact` | [`Artifact`]: something a step made, with its kind |
//! | `gaia:tree` | [`Tree`]: a stored tree plus owners and modes for its paths |
//! | `gaia:file-entry` | [`FileEntry`]: one path's mode and owner |
//! | `gaia:image` | [`Image`]: an image, its size and its partitions |
//! | `gaia:partition` | [`Partition`]: one partition of an image |
//! | `gaia:target` | [`Target`]: architecture, CPU and OS |
//! | `gaia:size` | [`Size`]: a size in bytes |
//! | `gaia:layer` | [`Layer`]: one owner's tree in an image |
//! | `gaia:placement` | [`Placement`]: one install entry of a component |
//! | `gaia:account` | [`Account`]: a user a component needs |
//! | `gaia:account-group` | [`AccountGroup`]: a group a component needs |
//! | `gaia:package-control` | [`Control`]: what a package says about itself |
//!
//! Kinds are open ([`ArtifactKind`], [`ImageKind`], [`PartitionRole`],
//! [`Arch`], [`Os`]): a known name, or `Other` with any name, so a `.uf2` or
//! `.ipk` output needs no type change.
//!
//! Every type is `serde` serializable: that is its value form for reports
//! and for anything stored. An open kind serializes as its plain name.

mod image;
mod kind;
mod layer;
mod object;
mod package;
mod target;

pub use image::{Image, ImageKind, Partition, PartitionRole, Size};
pub use layer::{Account, AccountGroup, Layer, Owned, Placement, owned_digest};
pub use object::{Artifact, ArtifactKind, FileEntry, ObjectKind, StoreRef, Tree};
pub use package::Control;
pub use target::{Arch, Os, Target};

/// Every type's key, port types first, then the kinds and enums inside them.
pub const KEYS: [&str; 19] = [
    "gaia:object",
    "gaia:artifact",
    "gaia:tree",
    "gaia:file-entry",
    "gaia:image",
    "gaia:partition",
    "gaia:target",
    "gaia:size",
    "gaia:layer",
    "gaia:placement",
    "gaia:account",
    "gaia:account-group",
    "gaia:package-control",
    "gaia:object-kind",
    "gaia:artifact-kind",
    "gaia:image-kind",
    "gaia:partition-role",
    "gaia:arch",
    "gaia:os",
];
