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
//! | `gaia:placements` | [`Placements`]: a component's install entries from one tree |
//! | `gaia:account` | [`Account`]: a user a component needs |
//! | `gaia:account-group` | [`AccountGroup`]: a group a component needs |
//! | `gaia:package-control` | [`Control`]: what a package says about itself |
//! | `gaia:disk-layout` | [`DiskLayout`]: a disk's table and the board's partitions |
//! | `gaia:disk-table` | [`DiskTable`]: a partition table's scheme and bounds |
//! | `gaia:disk-partition` | [`DiskPartition`]: one partition of a disk |
//! | `gaia:raw-blob` | [`RawBlob`]: bytes written at a fixed offset |
//! | `gaia:os-config` | [`OsConfig`]: an OS build's fragments, input trees and overrides |
//! | `gaia:buildroot-fragment` | [`ConfigFragment`]: a block of Buildroot config |
//! | `gaia:buildroot-host` | [`BuildrootHost`]: make, CPUs, jobs and the download cache |
//! | `gaia:kernel-contract` | [`KernelContract`]: what a kernel must have built in, by a file in an input tree |
//! | `gaia:source-override` | [`SourceOverride`]: a package built from an input tree |
//!
//! Kinds are open ([`ArtifactKind`], [`ImageKind`], [`PartitionRole`],
//! [`Arch`], [`Os`]): a known name, or `Other` with any name, so a `.uf2` or
//! `.ipk` output needs no type change.
//!
//! Artifacts, images and trees fan out: each has a copy conversion
//! ([`NODES`]) the planner inserts where several nodes take one by value.
//!
//! Every type is `serde` serializable: that is its value form for reports
//! and for anything stored. An open kind serializes as its plain name.

mod copies;
mod disk;
mod image;
mod kind;
mod layer;
mod object;
mod os;
mod package;
mod target;

pub use disk::{DiskLayout, DiskPartition, DiskTable, RawBlob, TableScheme};
pub use image::{Image, ImageKind, Partition, PartitionRole, Size};
pub use layer::{Account, AccountGroup, Layer, Owned, Placement, Placements, owned_digest};
pub use object::{Artifact, ArtifactKind, FileEntry, ObjectKind, StoreRef, Tree};
pub use os::{BuildrootHost, ConfigFragment, KernelContract, OsConfig, SourceOverride};
pub use package::Control;
pub use target::{Arch, Os, Target};

/// The copy conversions of the port types (see `copies`), for a registry.
pub const NODES: [&daedalus::prelude::NodeRecord; 3] = [
    &copies::COPY_ARTIFACT_NODE,
    &copies::COPY_IMAGE_NODE,
    &copies::COPY_TREE_NODE,
];

/// Every type's key, port types first, then the kinds and enums inside them.
pub const KEYS: [&str; 30] = [
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
    "gaia:placements",
    "gaia:account",
    "gaia:account-group",
    "gaia:package-control",
    "gaia:disk-layout",
    "gaia:disk-table",
    "gaia:disk-partition",
    "gaia:raw-blob",
    "gaia:os-config",
    "gaia:buildroot-fragment",
    "gaia:buildroot-host",
    "gaia:source-override",
    "gaia:kernel-contract",
    "gaia:object-kind",
    "gaia:artifact-kind",
    "gaia:image-kind",
    "gaia:partition-role",
    "gaia:table-scheme",
    "gaia:arch",
    "gaia:os",
];

/// The names a `[generate]` tool's argument may hold: its source tree, the
/// board's description file, the directory it writes into, the slot's id.
pub const GENERATE_NAMES: [&str; 4] = ["@source@", "@description@", "@out@", "@slot@"];

/// The names an argument of a `[generate.image]` or `[generate.bundle]`
/// run may hold: the tool's source tree, the board's description file, the
/// payload directory, the slot's id, the image's size in bytes and the file
/// to write; and, where the recipe gives them, the slot's root image, its
/// verity table, a signing key file, and the product's name and version.
pub const GENERATE_IMAGE_NAMES: [&str; 11] = [
    "@source@",
    "@description@",
    "@payload@",
    "@slot@",
    "@size@",
    "@out@",
    "@rootfs@",
    "@verity-table@",
    "@key@",
    "@product@",
    "@version@",
];

/// The `@name@` words of `text` that are none of [`GENERATE_NAMES`].
pub fn unknown_generate_names(text: &str) -> Vec<String> {
    unknown_names(text, &GENERATE_NAMES)
}

/// The `@name@` words of `text` that are none of `names`. A word between
/// two `@` that is no name (lowercase letters, digits and `-`) is not one:
/// `user@host` and a path with an `@` in it are left alone.
pub fn unknown_names(text: &str, names: &[&str]) -> Vec<String> {
    let mut unknown = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('@') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('@') else { break };
        let word = &after[..end];
        let whole = format!("@{word}@");
        let is_name = !word.is_empty()
            && word
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if is_name && !names.contains(&whole.as_str()) {
            unknown.push(whole);
        }
        rest = if is_name {
            &after[end + 1..]
        } else {
            &after[end..]
        };
    }
    unknown
}
