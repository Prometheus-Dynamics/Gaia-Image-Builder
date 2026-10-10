//! When the target tree of an output dir is already finalized.
//!
//! `target-finalize` is a phony target: every `make` that reaches it copies
//! each package's per-package target tree into `target/` (and host-finalize
//! patches every ELF file of `host/`), which takes most of a `make` with
//! nothing to build. Once a finalize has run on a tree with every package
//! installed and the config unchanged, the marker records that. The next
//! build of the same tree skips the finalize, because nothing it would write
//! has changed since.
//!
//! The marker is removed whenever the tree could change in a way the finalize
//! does not account for: a make that may build a package (anything but a
//! complete tree with nothing to build), and the image feed being applied to
//! `target/` (the feed reverts nothing when the finalize runs again, so a fed
//! tree is not the finalized one).
use super::*;

/// Present while the tree is the one the last finalize left.
pub(crate) const FINALIZED_MARKER: &str = ".gaia-target-finalized";

/// No finalize is current in `output_dir`.
pub(crate) fn invalidate_finalized(output_dir: &Path) {
    let _ = fs::remove_file(output_dir.join(FINALIZED_MARKER));
}

/// Records that a finalize of the tree (with `config_digest`) just succeeded.
pub(crate) fn record_finalized(output_dir: &Path, config_digest: &str) {
    let _ = fs::write(
        output_dir.join(FINALIZED_MARKER),
        format!("{config_digest}\n"),
    );
}

/// Whether the last recorded finalize was for `config_digest`.
pub(crate) fn finalized_for(output_dir: &Path, config_digest: &str) -> bool {
    fs::read_to_string(output_dir.join(FINALIZED_MARKER))
        .is_ok_and(|recorded| recorded.trim() == config_digest)
}

/// Whether every package of `graph` that has a build directory is installed:
/// a make would build nothing.
pub(crate) fn all_packages_installed(output_dir: &Path, graph: &PackageGraph) -> bool {
    graph.package_names().all(|name| {
        graph
            .get(name)
            .and_then(|package| package.stamp_dir.as_deref())
            .is_none_or(|stamp_dir| {
                output_dir
                    .join(stamp_dir)
                    .join(".stamp_installed")
                    .is_file()
            })
    })
}
