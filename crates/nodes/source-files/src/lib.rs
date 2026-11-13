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
//! - **`gaia.source.input`**: a file the command line names (`--input`),
//!   stored as one file under its name in a tree, keyed on its content and
//!   that name, not on its path. A changed file is a new step.
//! - **`gaia.source.download`**: a URL fetched with the `curl` program
//!   given and checked against the lock's digest, keyed on that digest. A
//!   download that stops part way resumes on the next run from what it got.
//!   A digest mismatch fails, naming both digests, and the partial file is
//!   dropped.

mod archive;
mod download;
mod files;
mod input;
mod path;

pub use archive::source_archive;
pub use download::{partial_dir, source_download};
pub use input::source_input;
pub use path::source_path;

/// The libraries this node links, pinned in its Cargo.toml. Its step key names
/// them, and `tests/libs.rs` checks each version against the pin.
pub const LIBS: &[(&str, &str)] = &[
    ("flate2", "1.1.11"),
    ("miniz_oxide", "0.9.1"),
    ("tar", "0.4.46"),
    ("xz2", "0.1.7"),
    ("lzma-sys", "0.1.20"),
];

/// libzstd, as the linked library reports its version (zstd-safe reads it at
/// run time). The step key names it with [`LIBS`].
pub fn runtime_libs() -> [(&'static str, &'static str); 1] {
    [("libzstd", zstd::zstd_safe::version_string())]
}

use daedalus::prelude::NodeRecord;

/// The nodes, for a registry.
pub const NODES: [&NodeRecord; 4] = [
    &input::SOURCE_INPUT_NODE,
    &path::SOURCE_PATH_NODE,
    &archive::SOURCE_ARCHIVE_NODE,
    &download::SOURCE_DOWNLOAD_NODE,
];
