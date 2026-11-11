//! Sources that are files: a local directory, an archive, a download.
//!
//! - **`gaia.source.path`**: a local directory as a stored tree, keyed on
//!   its tree digest. Files, directories and symlinks are copied as they
//!   are, modes included.
//! - **`gaia.source.archive`**: a stored tar archive (plain, gzip, xz or
//!   zstd, told apart by their first bytes) unpacked to a tree, keyed on the
//!   archive's digest and the leading components stripped. Nothing is
//!   written outside the tree: a member with an absolute path, a `..`, or a
//!   path through a symlink is refused. Modes are normalized: files 0644, or
//!   0755 when any execute bit is set; directories 0755.
//! - **`gaia.source.download`**: a URL fetched with the `curl` program
//!   given and checked against the lock's digest, keyed on that digest. A
//!   download that stops part way resumes on the next run from what it got.
//!   A digest mismatch fails, naming both digests, and the partial file is
//!   dropped.

mod archive;
mod download;
mod files;
mod path;

pub use archive::source_archive;
pub use download::{partial_dir, source_download};
pub use path::source_path;

use daedalus::prelude::NodeRecord;

/// The nodes, for a registry.
pub const NODES: [&NodeRecord; 3] = [
    &path::SOURCE_PATH_NODE,
    &archive::SOURCE_ARCHIVE_NODE,
    &download::SOURCE_DOWNLOAD_NODE,
];
