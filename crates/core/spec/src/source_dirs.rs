//! Where a source's files live on disk, shared by config substitution
//! (`${source.<id>.path}`), the planner and the executor.

use std::path::PathBuf;

use crate::{ResolvedBuildSpec, SourceDefinition, SourceSpec, WorkspaceSpec};

/// Directory a source's files are read from when it is built from:
/// - path source: its directory (canonical when it exists);
/// - git, archive and download sources: `<build_dir>/sources/<id>`, where
///   the source is materialized.
pub fn source_materialized_dir(workspace: &WorkspaceSpec, source: &SourceSpec) -> PathBuf {
    let in_workspace = |path: &str| {
        let candidate = PathBuf::from(path);
        if candidate.is_absolute() {
            candidate
        } else {
            PathBuf::from(&workspace.root_dir).join(candidate)
        }
    };
    match &source.definition {
        SourceDefinition::Path(path) => {
            let resolved = in_workspace(&path.path);
            std::fs::canonicalize(&resolved).unwrap_or(resolved)
        }
        SourceDefinition::Git(_) | SourceDefinition::Archive(_) | SourceDefinition::Download(_) => {
            in_workspace(&workspace.build_dir)
                .join("sources")
                .join(source.id.as_str())
        }
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
            }),
        )
    }

    #[test]
    fn git_sources_live_under_the_build_dir_and_imports_at_their_root() {
        let mut spec = ResolvedBuildSpec::new("dirs");
        spec.workspace.root_dir = "/work".into();
        spec.workspace.build_dir = "build".into();
        let orion = git_source("orion");
        assert_eq!(
            source_checkout_dir(&spec, &orion),
            PathBuf::from("/work/build/sources/orion")
        );
        spec.selection.import_sources.push(ImportSourceSpec {
            id: "orion".into(),
            root: "/work/.gaia/cache/import-sources/orion-abc".into(),
            identity: "git:repo@abc".into(),
            contributes: Vec::new(),
        });
        assert_eq!(
            source_checkout_dir(&spec, &orion),
            PathBuf::from("/work/.gaia/cache/import-sources/orion-abc")
        );
        assert_eq!(
            source_materialized_dir(&spec.workspace, &orion),
            PathBuf::from("/work/build/sources/orion")
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
