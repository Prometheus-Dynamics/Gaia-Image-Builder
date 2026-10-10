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

/// Present in `host/` after a `host-finalize` copy of the per-package host
/// trees succeeded; its content is the sorted package list that copy covered.
/// The make rule installed by `buildroot_patches` skips the copy while the
/// marker is newer than every installed stamp and everything under
/// `per-package/`, and lists the same packages.
pub(crate) const HOST_FINALIZED_MARKER: &str = ".gaia-host-finalized";

/// The host copy is out of date: the next `host-finalize` copies again. Called
/// wherever Gaia changes what the copy would read, and before a make whose
/// run changes it (see `run_buildroot_with`).
pub(crate) fn invalidate_host_finalized(output_dir: &Path) {
    let _ = fs::remove_file(output_dir.join("host").join(HOST_FINALIZED_MARKER));
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

/// Whether the `host-finalize` marker is newer than every installed stamp
/// under `build/`, as the make rule checks (see [`HOST_FINALIZED_MARKER`]).
pub(crate) fn host_finalized_current(output_dir: &Path) -> bool {
    let Ok(marker) = fs::metadata(output_dir.join("host").join(HOST_FINALIZED_MARKER))
        .and_then(|metadata| metadata.modified())
    else {
        return false;
    };
    let Ok(builds) = fs::read_dir(output_dir.join("build")) else {
        return true;
    };
    !builds.flatten().any(|build| {
        fs::read_dir(build.path()).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with(".stamp_")
                    && name.ends_with("installed")
                    && entry
                        .metadata()
                        .and_then(|metadata| metadata.modified())
                        .is_ok_and(|modified| modified > marker)
            })
        })
    })
}

/// Moves the `host-finalize` marker after `at`: refreshing the stamps of
/// unchanged packages (`refresh_current_stamps`) changes no host file, so a
/// copy that was current stays current.
pub(crate) fn keep_host_finalized_after(output_dir: &Path, at: std::time::SystemTime) {
    let marker = output_dir.join("host").join(HOST_FINALIZED_MARKER);
    if let Ok(file) = fs::File::options().append(true).open(marker) {
        let _ = file.set_modified(at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    #[test]
    fn the_host_marker_stays_current_across_a_stamp_refresh_only_when_moved() {
        let root = std::env::temp_dir().join(format!(
            "gaia-host-finalized-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let stamps = root.join("build/zlib-1.3");
        fs::create_dir_all(&stamps).expect("stamps");
        fs::create_dir_all(root.join("host")).expect("host");
        let stamp = stamps.join(".stamp_host_installed");
        let marker = root.join("host").join(HOST_FINALIZED_MARKER);
        let at = |path: &Path, time: SystemTime| {
            fs::File::create(path)
                .expect("file")
                .set_modified(time)
                .expect("mtime");
        };
        let now = SystemTime::now();
        assert!(!host_finalized_current(&root), "no marker");
        at(&stamp, now - Duration::from_secs(60));
        at(&marker, now - Duration::from_secs(30));
        assert!(host_finalized_current(&root));

        // A refresh moves the stamp past the marker...
        at(&stamp, now);
        assert!(!host_finalized_current(&root));
        // ...and moving the marker after it keeps the copy current.
        keep_host_finalized_after(&root, now + Duration::from_secs(1));
        assert!(host_finalized_current(&root));

        // A later install (a rebuilt package) makes it stale again.
        at(&stamp, now + Duration::from_secs(5));
        assert!(!host_finalized_current(&root));
        let _ = fs::remove_dir_all(root);
    }
}
