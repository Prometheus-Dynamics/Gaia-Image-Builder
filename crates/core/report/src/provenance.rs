use gaia_exec::ExecutionOutcome;
use gaia_spec::ResolvedBuildSpec;
use std::path::PathBuf;

use crate::masking::{mask_pairs, mask_value};
use crate::model::{
    ArtifactInstallIdentityRecord, BackendStateRecord, PrecedenceLayerReport, ProvenanceReport,
};
use crate::state::{
    artifact_spec_state_path, build_artifact_output_metadata, image_contract,
    output_hygiene_warnings, read_backend_state, rollback_domains, runtime_state_dir,
};

pub fn render_provenance(spec: &ResolvedBuildSpec, outcome: &ExecutionOutcome) -> ProvenanceReport {
    let mut source_providers = Vec::new();
    for source in &spec.sources {
        let provider = format!("{:?}", source.provider_kind());
        if !source_providers.contains(&provider) {
            source_providers.push(provider);
        }
    }

    let mut artifact_providers = Vec::new();
    for artifact in &spec.artifacts {
        let provider = format!("{:?}", artifact.provider_kind());
        if !artifact_providers.contains(&provider) {
            artifact_providers.push(provider);
        }
    }

    ProvenanceReport {
        build_name: spec.identity.display_name.clone(),
        build_version: spec.identity.version.clone(),
        build_branch: spec.metadata.branch.clone(),
        build_target: spec.metadata.target.clone(),
        build_profile: spec.metadata.profile.clone(),
        selected_build_file: spec.selection.selected_build_file.clone(),
        selected_preset: spec.selection.selected_preset.clone(),
        selected_inputs: mask_pairs(&spec.selection.selected_inputs, &spec.reporting),
        selected_env_files: spec.selection.env_files.clone(),
        selected_env_overrides: mask_pairs(&spec.selection.env_overrides, &spec.reporting),
        precedence_order: spec.selection.precedence_order.clone(),
        precedence_layers: spec
            .policy
            .precedence
            .layers
            .iter()
            .map(|layer| PrecedenceLayerReport {
                source: format!("{:?}", layer.source),
                applies_to: layer
                    .applies_to
                    .iter()
                    .map(|target| format!("{:?}", target))
                    .collect(),
            })
            .collect(),
        explicit_overrides: spec
            .selection
            .explicit_overrides
            .iter()
            .map(|(key, value)| {
                if let Some(env_key) = key.strip_prefix("env.") {
                    (key.clone(), mask_value(env_key, value, &spec.reporting))
                } else {
                    (key.clone(), value.clone())
                }
            })
            .collect(),
        metadata_labels: spec.metadata.labels.clone(),
        product_family: spec.metadata.product.family.clone(),
        product_name: spec.metadata.product.name.clone(),
        product_sku: spec.metadata.product.sku.clone(),
        identity_project: spec.provenance.identity.project.clone(),
        identity_vendor: spec.provenance.identity.vendor.clone(),
        identity_channel: spec.provenance.identity.channel.clone(),
        identity_labels: spec.provenance.identity.labels.clone(),
        rollback_on_error: spec.policy.failure.rollback_on_error,
        preserve_failed_outputs: spec.policy.failure.preserve_failed_outputs,
        rollback_domains: rollback_domains(spec),
        source_providers,
        artifact_providers,
        artifact_install_identities: spec
            .artifacts
            .iter()
            .filter_map(|artifact| {
                artifact
                    .install_identity
                    .as_ref()
                    .map(|identity| ArtifactInstallIdentityRecord {
                        artifact_id: artifact.id.as_str().to_string(),
                        install_name: identity.install_name.clone(),
                        install_class: identity.install_class.as_str().to_string(),
                        destination_hint: identity.destination_hint.clone(),
                    })
            })
            .collect(),
        artifact_output_metadata: spec
            .artifacts
            .iter()
            .map(build_artifact_output_metadata)
            .collect(),
        image_provider: format!("{:?}", spec.image.provider_kind()),
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
        image_output_collect_dirs: outcome
            .image_results
            .iter()
            .filter_map(|result| result.collect_dir.as_ref())
            .map(|path| path.display().to_string())
            .collect(),
        image_output_archives: outcome
            .image_results
            .iter()
            .filter_map(|result| result.archive_path.as_ref())
            .map(|path| path.display().to_string())
            .collect(),
        output_hygiene_warnings: output_hygiene_warnings(spec),
        source_backend_states: spec
            .sources
            .iter()
            .map(|source| BackendStateRecord {
                id: source.id.as_str().to_string(),
                state: read_backend_state(
                    &PathBuf::from(&spec.workspace.build_dir)
                        .join("sources")
                        .join(source.id.as_str())
                        .join(".gaia-source-state.txt"),
                ),
            })
            .collect(),
        artifact_backend_states: spec
            .artifacts
            .iter()
            .map(|artifact| BackendStateRecord {
                id: artifact.id.as_str().to_string(),
                state: read_backend_state(&artifact_spec_state_path(artifact)),
            })
            .collect(),
        image_backend_states: outcome
            .image_results
            .iter()
            .map(|result| BackendStateRecord {
                id: result.provider_id.clone(),
                state: result
                    .collect_dir
                    .as_ref()
                    .map(|dir| read_backend_state(&dir.join(".gaia-image-state.txt")))
                    .unwrap_or_default(),
            })
            .collect(),
        install_backend_states: spec
            .install
            .entries
            .iter()
            .map(|install| BackendStateRecord {
                id: install.id.as_str().to_string(),
                state: read_backend_state(
                    &runtime_state_dir(spec).join(format!("install-{}.state", install.id.as_str())),
                ),
            })
            .collect(),
        stage_file_backend_states: spec
            .stage
            .files
            .iter()
            .map(|file| BackendStateRecord {
                id: file.id.as_str().to_string(),
                state: read_backend_state(
                    &runtime_state_dir(spec).join(format!("stage-file-{}.state", file.id.as_str())),
                ),
            })
            .collect(),
        stage_env_set_backend_states: spec
            .stage
            .env_sets
            .iter()
            .map(|env_set| BackendStateRecord {
                id: env_set.id.as_str().to_string(),
                state: read_backend_state(
                    &runtime_state_dir(spec)
                        .join(format!("stage-env-{}.state", env_set.id.as_str())),
                ),
            })
            .collect(),
        stage_service_backend_states: spec
            .stage
            .services
            .iter()
            .map(|service| BackendStateRecord {
                id: service.id.as_str().to_string(),
                state: read_backend_state(
                    &runtime_state_dir(spec)
                        .join(format!("stage-service-{}.state", service.id.as_str())),
                ),
            })
            .collect(),
        image_assembly_backend_states: spec
            .image
            .assembly
            .as_ref()
            .map(|_| {
                vec![BackendStateRecord {
                    id: "image:assembly".to_string(),
                    state: read_backend_state(
                        &runtime_state_dir(spec).join(gaia_spec::IMAGE_ASSEMBLY_STATE_FILE_NAME),
                    ),
                }]
            })
            .unwrap_or_default(),
        checkpoint_backend_states: spec
            .checkpoints
            .points
            .iter()
            .map(|checkpoint| BackendStateRecord {
                id: checkpoint.id.as_str().to_string(),
                state: read_backend_state(
                    &runtime_state_dir(spec)
                        .join(format!("checkpoint-{}.state", checkpoint.id.as_str())),
                ),
            })
            .collect(),
        completed_operation_ids: outcome
            .completed_ids
            .iter()
            .map(|id| id.as_str().to_string())
            .collect(),
    }
}
