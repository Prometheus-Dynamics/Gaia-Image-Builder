//! Reuse inputs contributed by config files imported from a git source.
//!
//! Values a source-imported layer sets are already part of the spec parts
//! that operation fingerprints hash, and `@self` paths into a checkout carry
//! the revision in the cache directory name. This adds the import source's
//! identity (commit, or local override digest) to the operations whose items
//! the layer declared, so moving the import to another revision rebuilds
//! what came from it even when the resulting spec values are unchanged.

use crate::OperationKind;
use gaia_spec::ResolvedBuildSpec;

/// Identities of the import sources that contributed to `kind`, or `None`
/// when no source-imported file did (keeping existing fingerprints).
pub(crate) fn import_source_signature(
    spec: &ResolvedBuildSpec,
    kind: &OperationKind,
) -> Option<String> {
    let key = contribution_key(kind)?;
    let identities = spec
        .selection
        .import_sources
        .iter()
        .filter(|source| source.contributes_to(&key))
        .map(|source| format!("{}={}", source.id, source.identity))
        .collect::<Vec<_>>();
    (!identities.is_empty()).then(|| identities.join("|"))
}

fn contribution_key(kind: &OperationKind) -> Option<String> {
    Some(match kind {
        OperationKind::MaterializeSource { source_id } => {
            format!("source:{}", source_id.as_str())
        }
        OperationKind::BuildArtifact { artifact_id } => {
            format!("artifact:{}", artifact_id.as_str())
        }
        OperationKind::InstallArtifact { install_id, .. } => {
            format!("install:{}", install_id.as_str())
        }
        OperationKind::RenderStageFile { item_id } => format!("stage-file:{}", item_id.as_str()),
        OperationKind::RenderStageEnvSet { item_id } => {
            format!("stage-env-set:{}", item_id.as_str())
        }
        OperationKind::RenderStageService { item_id } => {
            format!("stage-service:{}", item_id.as_str())
        }
        OperationKind::PrepareImage | OperationKind::BuildImage | OperationKind::AssembleImage => {
            "image".to_string()
        }
        OperationKind::ResolveBuild
        | OperationKind::CaptureCheckpoint { .. }
        | OperationKind::EmitReport => return None,
    })
}
