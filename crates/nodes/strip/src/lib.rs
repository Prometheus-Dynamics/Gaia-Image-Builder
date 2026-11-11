//! Stripping and runtime closure: ELF files stripped with a pinned tool, and
//! the shared libraries of a tree that no program in it needs.
//!
//! - **`gaia.strip.elf`**: a tree's ELF files stripped (`--strip-unneeded` for
//!   executables and shared objects, `--strip-debug` for relocatable objects)
//!   by the strip tool the graph names. Gives the stripped tree.
//! - **`gaia.strip.unused-libs`**: a report of the tree's `lib*.so*` libraries
//!   that no program needs, the ones a program needs, and the names missing
//!   from the tree. Changes nothing.
//! - **`gaia.strip.prune-libs`**: the same analysis, and the tree without the
//!   unused libraries (and the symlinks that led to them).
//!
//! Only what the tree holds is read. Nothing is looked up on the build host,
//! and nothing is run except the strip tool.

mod elf;
mod libs;
mod strip;
mod tree;

pub use libs::{prune_libs, unused_libs};
pub use strip::strip_elf;

use daedalus::prelude::NodeRecord;

/// The nodes, for a registry.
pub const NODES: [&NodeRecord; 3] = [
    &strip::STRIP_ELF_NODE,
    &libs::UNUSED_LIBS_NODE,
    &libs::PRUNE_LIBS_NODE,
];
