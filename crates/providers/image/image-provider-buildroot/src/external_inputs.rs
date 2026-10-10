//! The `BR2_EXTERNAL` files a build uses, by content, for three decisions:
//! which packages a changed external file rebuilds (the clean planning), the
//! package cache keys of the packages those files touch, and the record of
//! the files the last build used.
//!
//! The mapping from files to packages is shared with the plan crate (see
//! `gaia_image_providers::external_tree_files`), so `gaia preview` and a run
//! name the same packages.
use super::*;
use gaia_image_providers::{
    ExternalChanges, ExternalFile, ExternalTree, encode_external_state, external_changes,
    external_package_digests, external_tree_files, external_trees,
};

/// The `BR2_EXTERNAL` trees of the spec.
pub(crate) fn external_trees_of(spec: &ResolvedBuildSpec) -> Vec<ExternalTree> {
    let external_tree = match &spec.image.definition {
        ImageDefinition::Buildroot(buildroot) => buildroot.external_tree.as_deref(),
        _ => None,
    };
    external_trees(&spec.workspace, external_tree)
}

/// The files of the spec's external trees (see `external_tree_files`).
pub(crate) fn current_external_files(spec: &ResolvedBuildSpec) -> BTreeMap<String, ExternalFile> {
    external_tree_files(&external_trees_of(spec))
}

/// The message a tree with no record of its external files gets: its
/// current files become the baseline, so nothing is rebuilt for them.
pub(crate) const NO_RECORD_BASELINE: &str = "buildroot external files: no earlier record; recorded the current files as the baseline (use --rebuild-package to force)";

/// The digest of the external files that touch each package, for the package
/// cache keys.
pub(crate) fn external_package_key_digests(spec: &ResolvedBuildSpec) -> BTreeMap<String, String> {
    external_package_digests(&current_external_files(spec))
}

/// The files the last build recorded.
const EXTERNAL_FILES_STATE: &str = ".gaia-buildroot-external-files.state";

/// The external files that changed since the last build, against `current`.
///
/// A tree built before this record existed has no record of its external
/// files, so what changed is unknown. The current files are adopted as the
/// baseline instead: nothing is rebuilt now, and the run records the files
/// (see `write_external_files_state`), so the next run compares them.
pub(crate) fn external_changes_since_build(
    output_dir: &Path,
    current: &BTreeMap<String, ExternalFile>,
) -> ExternalChanges {
    match fs::read_to_string(output_dir.join(EXTERNAL_FILES_STATE)) {
        Ok(text) => external_changes(&gaia_image_providers::decode_external_state(&text), current),
        Err(_) => ExternalChanges {
            packages: BTreeSet::new(),
            unmapped: Vec::new(),
            reasons: if current.is_empty() {
                Vec::new()
            } else {
                vec![NO_RECORD_BASELINE.to_string()]
            },
        },
    }
}

pub(crate) fn write_external_files_state(
    output_dir: &Path,
    files: &BTreeMap<String, ExternalFile>,
) -> Result<(), ImageProviderError> {
    fs::write(
        output_dir.join(EXTERNAL_FILES_STATE),
        encode_external_state(files),
    )
    .map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to record Buildroot external files in '{}': {error}",
            output_dir.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_image_providers::{ExternalFile, external_package_digests};
    use std::collections::BTreeSet;

    fn file(digest: &str, packages: &[&str]) -> ExternalFile {
        ExternalFile {
            digest: digest.to_string(),
            packages: packages
                .iter()
                .map(|name| name.to_string())
                .collect::<BTreeSet<_>>(),
        }
    }

    fn graph(names: &[&str]) -> PackageGraph {
        let mut graph = PackageGraph::default();
        for name in names {
            graph.packages.insert(
                name.to_string(),
                PackageInfo {
                    kind: if name.starts_with("host-") {
                        "host"
                    } else {
                        "target"
                    }
                    .to_string(),
                    version: Some("1".to_string()),
                    stamp_dir: Some(format!("build/{name}-1")),
                    ..PackageInfo::default()
                },
            );
        }
        graph
    }

    fn decide(changes: &TreeChanges, graph: &PackageGraph) -> CleanPlan {
        let symbol_use = |_: &str| SymbolUse::default();
        decide_clean(CleanDecisionInput {
            built_before: true,
            changes,
            previous: Some(graph),
            current: Some(graph),
            symbol_use: &symbol_use,
            per_package: true,
        })
    }

    #[test]
    fn a_changed_external_mk_rebuilds_the_package_it_assigns() {
        // external.mk sets HOST_EROFS_UTILS_CONF_OPTS: the host package.
        let previous = BTreeMap::from([(
            "RAZE:external.mk".to_string(),
            file("sha256:one", &["host-erofs-utils"]),
        )]);
        let current = BTreeMap::from([(
            "RAZE:external.mk".to_string(),
            file("sha256:two", &["host-erofs-utils"]),
        )]);
        let changes = external_changes(&previous, &current);
        assert_eq!(
            changes.packages,
            BTreeSet::from(["host-erofs-utils".to_string()])
        );
        let tree = TreeChanges {
            override_changes: changes.packages.clone(),
            external_reasons: changes.reasons.clone(),
            ..TreeChanges::default()
        };
        let graph = graph(&["host-erofs-utils", "zlib"]);
        let CleanPlan::Packages(rebuild) = decide(&tree, &graph) else {
            panic!("expected a package rebuild");
        };
        assert!(rebuild.rebuild.contains("host-erofs-utils"), "{rebuild:?}");
        assert!(!rebuild.rebuild.contains("zlib"));
        assert!(
            rebuild
                .reasons
                .iter()
                .any(|line| line.contains("host-erofs-utils"))
        );
    }

    #[test]
    fn an_unmapped_external_change_is_a_full_clean() {
        let changes = TreeChanges {
            external_unmapped: vec!["RAZE:Config.in".to_string()],
            ..TreeChanges::default()
        };
        let CleanPlan::Full(reasons) = decide(&changes, &graph(&["zlib"])) else {
            panic!("expected a full clean");
        };
        assert_eq!(
            reasons,
            ["buildroot external file RAZE:Config.in changed and maps to no package"]
        );
    }

    #[test]
    fn a_changed_external_file_changes_only_its_packages_cache_keys() {
        let graph = graph(&["host-erofs-utils", "zlib"]);
        let key = |external: &BTreeMap<String, String>| {
            package_keys(&KeyInputs {
                buildroot_dir: Path::new("/nonexistent-buildroot"),
                output_dir: Path::new("/nonexistent-output"),
                graph: &graph,
                execution_identity: "host:test",
                external,
            })
        };
        let before = key(&BTreeMap::new());
        let after = key(&external_package_digests(&BTreeMap::from([(
            "RAZE:external.mk".to_string(),
            file("sha256:two", &["host-erofs-utils"]),
        )])));
        assert_ne!(before["host-erofs-utils"], after["host-erofs-utils"]);
        assert_eq!(before["zlib"], after["zlib"]);
    }

    #[test]
    fn a_tree_without_a_recorded_state_adopts_its_files_as_the_baseline() {
        // The real case: a `linux` fragment or a `.mk` assigning a kernel
        // package must not rebuild it just because no record exists yet.
        let current = BTreeMap::from([
            ("RAZE:external.mk".to_string(), file("sha256:x", &["linux"])),
            ("RAZE:Config.in".to_string(), file("sha256:c", &[])),
        ]);
        let dir = std::env::temp_dir().join(format!("gaia-external-state-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("dir");
        let _ = fs::remove_file(dir.join(EXTERNAL_FILES_STATE));
        let baseline = external_changes_since_build(&dir, &current);
        assert_eq!(baseline.packages, BTreeSet::new());
        assert!(baseline.unmapped.is_empty());
        assert_eq!(baseline.reasons, [NO_RECORD_BASELINE.to_string()]);
        // The run records the files; the next run compares against them.
        write_external_files_state(&dir, &current).expect("record");
        assert_eq!(
            external_changes_since_build(&dir, &current),
            ExternalChanges::default()
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_tree_without_external_files_has_no_baseline_message() {
        let dir = std::env::temp_dir().join(format!("gaia-external-empty-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("dir");
        let _ = fs::remove_file(dir.join(EXTERNAL_FILES_STATE));
        assert!(
            external_changes_since_build(&dir, &BTreeMap::new())
                .reasons
                .is_empty()
        );
        let _ = fs::remove_dir_all(dir);
    }
}
