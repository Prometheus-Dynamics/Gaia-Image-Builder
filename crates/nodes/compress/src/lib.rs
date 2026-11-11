//! Archives and compression, byte for byte the same on every host.
//!
//! - **`gaia.compress.tar`**: a stored tree as a tar. Members sorted by
//!   path, every timestamp 0, owners numeric (from the tree's entries, root
//!   otherwise) with no names, modes from the entries or the store.
//! - **`gaia.compress.xz`**, **`gaia.compress.zstd`**,
//!   **`gaia.compress.gzip`**: one artifact compressed. The level is a knob
//!   and part of the key. The thread count is a knob and is not: xz and
//!   zstd always run their multithreaded encoders, whose output does not
//!   depend on how many threads they get (xz splits into blocks set by the
//!   level, zstd into jobs set by the level). gzip has no multithreaded
//!   form and runs on one thread.
//!
//! The result's kind is the input's kind with the format after a `+`
//! (`archive+zstd`, `disk+xz`), so a reader knows what to undo.

mod codec;
mod tar_tree;

pub use codec::{compress_gzip, compress_xz, compress_zstd};
pub use tar_tree::tar_tree;

use daedalus::prelude::NodeRecord;

/// The nodes, for a registry.
pub const NODES: [&NodeRecord; 4] = [
    &tar_tree::TAR_TREE_NODE,
    &codec::COMPRESS_XZ_NODE,
    &codec::COMPRESS_ZSTD_NODE,
    &codec::COMPRESS_GZIP_NODE,
];
