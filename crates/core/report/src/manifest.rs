use gaia_exec::ExecutionOutcome;
use gaia_plan::ExecutionPlan;
use gaia_spec::ResolvedBuildSpec;
use std::path::PathBuf;

use crate::model::{
    ManifestArtifactRecord, ManifestCheckpointRecord, ManifestImageOutputRecord,
    ManifestInstallRecord, ManifestOperationRecord, ManifestReport, ManifestSourceRecord,
    ManifestStageEnvSetRecord, ManifestStageFileRecord, ManifestStageServiceRecord,
};
use crate::state::{
    artifact_spec_state_path, build_artifact_output_metadata, image_contract, read_backend_state,
    rollback_domains, runtime_state_dir,
};

pub fn render_manifest(spec: &ResolvedBuildSpec, plan: &ExecutionPlan) -> ManifestReport {
    ManifestReport {
        rollback_on_error: spec.policy.failure.rollback_on_error,
        preserve_failed_outputs: spec.policy.failure.preserve_failed_outputs,
        rollback_domains: rollback_domains(spec),
        image_feed_install_entries: spec
            .image
            .feed
            .install_entries
            .iter()
            .map(|id| id.as_str().to_string())
            .collect(),
        image_feed_stage_files: spec
            .image
            .feed
            .stage_files
            .iter()
            .map(|id| id.as_str().to_string())
            .collect(),
        image_feed_stage_env_sets: spec
            .image
            .feed
            .stage_env_sets
            .iter()
            .map(|id| id.as_str().to_string())
            .collect(),
        image_feed_stage_services: spec
            .image
            .feed
            .stage_services
            .iter()
            .map(|id| id.as_str().to_string())
            .collect(),
        image_contract: image_contract(spec),
        operations: plan
            .operations
            .iter()
            .map(|operation| ManifestOperationRecord {
                id: operation.id.as_str().to_string(),
                dependency_ids: operation
                    .depends_on
                    .iter()
                    .map(|dependency| dependency.as_str().to_string())
                    .collect(),
                optionality: operation.optionality.as_str().to_string(),
            })
            .collect(),
        sources: spec
            .sources
            .iter()
            .map(|source| ManifestSourceRecord {
                id: source.id.as_str().to_string(),
                provider: format!("{:?}", source.provider_kind()),
                backend_state: read_backend_state(
                    &PathBuf::from(&spec.workspace.build_dir)
                        .join("sources")
                        .join(source.id.as_str())
                        .join(".gaia-source-state.txt"),
                ),
            })
            .collect(),
        artifacts: spec
            .artifacts
            .iter()
            .map(|artifact| {
                let output_metadata = build_artifact_output_metadata(artifact);
                ManifestArtifactRecord {
                    id: artifact.id.as_str().to_string(),
                    provider: format!("{:?}", artifact.provider_kind()),
                    output_path: artifact.output.path.clone(),
                    resolved_identifier_kind: output_metadata.resolved_identifier_kind,
                    resolved_identifier: output_metadata.resolved_identifier,
                    produced_filename: output_metadata.produced_filename,
                    output_class: output_metadata.output_class,
                    build_tool: output_metadata.build_tool,
                    build_tool_version: output_metadata.build_tool_version,
                    install_name: artifact
                        .install_identity
                        .as_ref()
                        .map(|identity| identity.install_name.clone()),
                    install_class: artifact
                        .install_identity
                        .as_ref()
                        .map(|identity| identity.install_class.as_str().to_string()),
                    install_destination_hint: artifact
                        .install_identity
                        .as_ref()
                        .and_then(|identity| identity.destination_hint.clone()),
                    backend_state: read_backend_state(&artifact_spec_state_path(artifact)),
                }
            })
            .collect(),
        installs: spec
            .install
            .entries
            .iter()
            .map(|install| ManifestInstallRecord {
                id: install.id.as_str().to_string(),
                artifact_id: install.artifact.id.as_str().to_string(),
                dest: install.dest.clone(),
                backend_state: read_backend_state(
                    &runtime_state_dir(spec).join(format!("install-{}.state", install.id.as_str())),
                ),
            })
            .collect(),
        stage_files: spec
            .stage
            .files
            .iter()
            .map(|file| ManifestStageFileRecord {
                id: file.id.as_str().to_string(),
                src: file.src.clone(),
                dest: file.dest.clone(),
                origin: file.origin.as_str().to_string(),
                backend_state: read_backend_state(
                    &runtime_state_dir(spec).join(format!("stage-file-{}.state", file.id.as_str())),
                ),
            })
            .collect(),
        stage_env_sets: spec
            .stage
            .env_sets
            .iter()
            .map(|env_set| ManifestStageEnvSetRecord {
                id: env_set.id.as_str().to_string(),
                name: env_set.name.clone(),
                entry_count: env_set.entries.len(),
                backend_state: read_backend_state(
                    &runtime_state_dir(spec)
                        .join(format!("stage-env-{}.state", env_set.id.as_str())),
                ),
            })
            .collect(),
        stage_services: spec
            .stage
            .services
            .iter()
            .map(|service| ManifestStageServiceRecord {
                id: service.id.as_str().to_string(),
                name: service.name.clone(),
                unit_path: service.unit_path.clone(),
                backend_state: read_backend_state(
                    &runtime_state_dir(spec)
                        .join(format!("stage-service-{}.state", service.id.as_str())),
                ),
            })
            .collect(),
        image_outputs: Vec::new(),
        checkpoints: spec
            .checkpoints
            .points
            .iter()
            .map(|checkpoint| ManifestCheckpointRecord {
                id: checkpoint.id.as_str().to_string(),
                backend: checkpoint
                    .backend
                    .as_ref()
                    .map(|backend| backend.backend.clone()),
                anchor: checkpoint.anchor.as_str(),
                backend_state: read_backend_state(
                    &runtime_state_dir(spec)
                        .join(format!("checkpoint-{}.state", checkpoint.id.as_str())),
                ),
            })
            .collect(),
    }
}

pub fn render_manifest_with_outcome(
    spec: &ResolvedBuildSpec,
    plan: &ExecutionPlan,
    outcome: &ExecutionOutcome,
) -> ManifestReport {
    let mut manifest = render_manifest(spec, plan);
    manifest.image_outputs = outcome
        .image_results
        .iter()
        .map(|result| ManifestImageOutputRecord {
            provider_id: result.provider_id.clone(),
            image_contract: image_contract(spec),
            image_feed_install_entries: spec
                .image
                .feed
                .install_entries
                .iter()
                .map(|id| id.as_str().to_string())
                .collect(),
            image_feed_stage_files: spec
                .image
                .feed
                .stage_files
                .iter()
                .map(|id| id.as_str().to_string())
                .collect(),
            image_feed_stage_env_sets: spec
                .image
                .feed
                .stage_env_sets
                .iter()
                .map(|id| id.as_str().to_string())
                .collect(),
            image_feed_stage_services: spec
                .image
                .feed
                .stage_services
                .iter()
                .map(|id| id.as_str().to_string())
                .collect(),
            collect_dir: result
                .collect_dir
                .as_ref()
                .map(|path| path.display().to_string()),
            archive_path: result
                .archive_path
                .as_ref()
                .map(|path| path.display().to_string()),
            emit_report: result.emit_report,
            reused: result.reused,
            reuse_details: result.reuse_details.clone(),
            backend_state: result
                .collect_dir
                .as_ref()
                .map(|dir| read_backend_state(&dir.join(".gaia-image-state.txt")))
                .unwrap_or_default(),
        })
        .collect();
    manifest
}
