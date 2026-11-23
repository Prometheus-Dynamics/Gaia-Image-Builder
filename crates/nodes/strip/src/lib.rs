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
//! - **`gaia.strip.runtime-libs`**: a root without the toolchain runtime
//!   libraries (a fixed list, and the libraries a product names) that no ELF
//!   file in it needs, with a report of each removed and kept. What an image
//!   with an OS is made from goes through it.
//!
//! Only what the tree holds is read. Nothing is looked up on the build host,
//! and nothing is run except the strip tool.

mod elf;
mod libs;
pub mod runtime;
mod strip;
mod tree;

pub use libs::{prune_libs, unused_libs};
pub use runtime::runtime_libs;
pub use strip::strip_elf;

use daedalus::prelude::NodeRecord;

/// The nodes, for a registry.
pub const NODES: [&NodeRecord; 4] = [
    &strip::STRIP_ELF_NODE,
    &libs::UNUSED_LIBS_NODE,
    &libs::PRUNE_LIBS_NODE,
    &runtime::RUNTIME_LIBS_NODE,
];
