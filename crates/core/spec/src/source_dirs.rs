//! Where a source's files live on disk, shared by config substitution
//! (`${source.<id>.path}`), the planner and the executor.

use std::path::PathBuf;

use crate::{ResolvedBuildSpec, SourceDefinition, SourceSpec, WorkspaceSpec};

/// Directory a source's files are read from when it is built from:
/// - path source: its directory (canonical when it exists);
/// - git, archive and download sources: [`source_build_dir`], where the
///   source is materialized.
pub fn source_materialized_dir(workspace: &WorkspaceSpec, source: &SourceSpec) -> PathBuf {
    match &source.definition {
        SourceDefinition::Path(path) => {
            let resolved = in_workspace(workspace, &path.path);
            std::fs::canonicalize(&resolved).unwrap_or(resolved)
        }
        SourceDefinition::Git(_) | SourceDefinition::Archive(_) | SourceDefinition::Download(_) => {
            source_build_dir(workspace, source)
        }
    }
}

/// The build-dir location of a source Gaia materializes:
/// `<build_dir>/sources/<id>`, or `<build_dir>/sources/variants/<variant>/<id>`
/// for a source a variant checks out for itself. Path sources use it too,
/// for the link that points at their directory.
pub fn source_build_dir(workspace: &WorkspaceSpec, source: &SourceSpec) -> PathBuf {
    let sources = in_workspace(workspace, &workspace.build_dir).join("sources");
    match &source.variant_scope {
        Some(variant) => sources
            .join("variants")
            .join(variant)
            .join(source.id.as_str()),
        None => sources.join(source.id.as_str()),
    }
}

/// [`source_build_dir`] of the source with this id. A source the spec does
/// not declare has no variant scope, so it gets the shared location.
pub fn source_build_dir_by_id(spec: &ResolvedBuildSpec, source_id: &str) -> PathBuf {
    match spec
        .sources
        .iter()
        .find(|source| source.id.as_str() == source_id)
    {
        Some(source) => source_build_dir(&spec.workspace, source),
        None => in_workspace(&spec.workspace, &spec.workspace.build_dir)
            .join("sources")
            .join(source_id),
    }
}

fn in_workspace(workspace: &WorkspaceSpec, path: &str) -> PathBuf {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        candidate
    } else {
        PathBuf::from(&workspace.root_dir).join(candidate)
    }
}

/// The checkout directory `${source.<id>.path}` names: an import source's
/// checkout root (it exists once config is resolved), otherwise
/// [`source_materialized_dir`].
pub fn source_checkout_dir(spec: &ResolvedBuildSpec, source: &SourceSpec) -> PathBuf {
    spec.selection
        .import_sources
        .iter()
        .find(|import| import.id == source.id.as_str())
        .map(|import| PathBuf::from(&import.root))
        .unwrap_or_else(|| source_materialized_dir(&spec.workspace, source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GitSourceSpec, ImportSourceSpec, PathSourceSpec};

    fn git_source(id: &str) -> SourceSpec {
        SourceSpec::new(
            id,
            SourceDefinition::Git(GitSourceSpec {
                repo: "https://example.invalid/repo.git".into(),
                branch: None,
                tag: None,
                rev: None,
                subdir: None,
                update: false,
                refresh_policy: crate::SourceRefreshPolicySpec::Auto,
                pin_policy: crate::SourcePinPolicySpec::Floating,
                locked_commit: None,
                git_backend: crate::GitBackendSpec::Host,
            }),
        )
    }

    #[test]
    fn git_sources_live_under_the_build_dir_and_imports_at_their_root() {
        let mut spec = ResolvedBuildSpec::new("dirs");
        spec.workspace.root_dir = "/work".into();
        spec.workspace.build_dir = "build".into();
        let worker = git_source("worker");
        assert_eq!(
            source_checkout_dir(&spec, &worker),
            PathBuf::from("/work/build/sources/worker")
        );
        spec.selection.import_sources.push(ImportSourceSpec {
            id: "worker".into(),
            root: "/work/.gaia/cache/import-sources/worker-abc".into(),
            identity: "git:repo@abc".into(),
            contributes: Vec::new(),
        });
        assert_eq!(
            source_checkout_dir(&spec, &worker),
            PathBuf::from("/work/.gaia/cache/import-sources/worker-abc")
        );
        assert_eq!(
            source_materialized_dir(&spec.workspace, &worker),
            PathBuf::from("/work/build/sources/worker")
        );
    }

    #[test]
    fn path_sources_resolve_against_the_workspace_root() {
        let mut spec = ResolvedBuildSpec::new("dirs");
        spec.workspace.root_dir = "/nonexistent-gaia-root".into();
        let local = SourceSpec::new(
            "local",
            SourceDefinition::Path(PathSourceSpec {
                path: "vendor/local".into(),
                identity_ignore: Vec::new(),
                refresh_policy: crate::SourceRefreshPolicySpec::Never,
                pin_policy: crate::SourcePinPolicySpec::Locked,
            }),
        );
        assert_eq!(
            source_checkout_dir(&spec, &local),
            PathBuf::from("/nonexistent-gaia-root/vendor/local")
        );
    }
}
