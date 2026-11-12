//! `gaia.os.buildroot`'s caches, as nodes and the rules they follow.
//!
//! The Buildroot node (`gaia-os-buildroot`) takes a download directory and
//! config fragments. This crate fills the one from the lock and supplies the
//! other, and it explains the two things a cache can change:
//!
//! - **Downloads** (`downloads`): the download directory (`BR2_DL_DIR`) is
//!   made from the lock. Each locked file is copied from the store into the
//!   directory at `<package>/<file>`, the layout Buildroot reads, and checked
//!   against its digest. A file already there with the right digest is kept.
//!   With every file present, the build needs no network: Buildroot finds
//!   each source by its hash file and never fetches it.
//! - **ccache** (`ccache`): the fragment that turns on Buildroot's compiler
//!   cache at a directory. Products that name the same directory share it.
//!   The directory is not part of the output, so the key of a build has the
//!   directory only as the fragment's text.
//! - **Clean builds** (`rebuild`): a config change makes a new key, and the
//!   node rebuilds from nothing. It names the config options that changed
//!   between the previous and the current fragments, so the reason is in
//!   the run's output and not only in the key.
//! - **Interrupted make** (`interrupted`): a marker a make leaves behind
//!   when it is killed, and the package build directories it was in the
//!   middle of, which are removed so the next make builds them again from
//!   the start. Finished packages keep their stamps and are not built again.
//!
//! Nothing here is a scheduler or a retry loop. Retries are Daedalus step
//! policy; the store is the only state a node keeps.

mod ccache;
mod downloads;
pub mod interrupted;
mod rebuild;

pub use ccache::fragment as ccache_fragment;
pub use downloads::{Filled, LockedFile, fill, locked_file, os_buildroot_downloads};
pub use rebuild::{reasons, settings};

use daedalus::prelude::NodeRecord;

/// The nodes, for a registry.
pub const NODES: [&NodeRecord; 3] = [
    &ccache::OS_BUILDROOT_CCACHE_NODE,
    &downloads::OS_BUILDROOT_DOWNLOADS_NODE,
    &rebuild::OS_BUILDROOT_REBUILD_REASON_NODE,
];
