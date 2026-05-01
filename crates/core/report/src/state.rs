use gaia_spec::{ImageDefinition, ResolvedBuildSpec};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::model::ArtifactOutputMetadataRecord;

pub(crate) fn rollback_domains(spec: &ResolvedBuildSpec) -> Vec<String> {
    spec.policy
        .failure
        .rollback_domains
        .iter()
        .map(|domain| domain.as_str().to_string())
        .collect()
}

pub(crate) fn image_contract(spec: &ResolvedBuildSpec) -> BTreeMap<String, String> {
    let mut contract = BTreeMap::new();
    match &spec.image.definition {
        ImageDefinition::Buildroot(buildroot) => {
            contract.insert("provider".into(), "buildroot".into());
            contract.insert(
                "config_fragments".into(),
                buildroot.config_fragments.join(","),
            );
            contract.insert(
                "config_overrides".into(),
                buildroot
                    .config_overrides
                    .iter()
                    .map(|(key, value)| format!("{key}={value}"))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            contract.insert(
                "external_tree_mode".into(),
                buildroot.external_tree_mode.as_str().to_string(),
            );
            contract.insert(
                "expected_images".into(),
                buildroot
                    .expected_images
                    .iter()
                    .map(|image| {
                        format!(
                            "{}:{}:{}",
                            image.name,
                            image.format.as_str(),
                            if image.required {
                                "required"
                            } else {
                                "optional"
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        ImageDefinition::StartingPoint(starting_point) => {
            contract.insert("provider".into(), "starting-point".into());
            contract.insert(
                "rootfs_validation_mode".into(),
                starting_point.rootfs_validation_mode.as_str().to_string(),
            );
            contract.insert(
                "output_mode".into(),
                starting_point.output_mode.as_str().to_string(),
            );
        }
    }
    contract
}

pub(crate) fn artifact_spec_state_path(artifact: &gaia_spec::ArtifactSpec) -> PathBuf {
    let output_path = artifact.output.as_path();
    if output_path.is_dir() {
        output_path.join(".gaia-state.txt")
    } else {
        output_path.with_extension("gaia-state.txt")
    }
}

pub(crate) fn read_backend_state(path: &Path) -> BTreeMap<String, String> {
    fs::read_to_string(path)
        .ok()
        .map(|contents| parse_backend_state(&contents))
        .unwrap_or_default()
}

pub(crate) fn build_artifact_output_metadata(
    artifact: &gaia_spec::ArtifactSpec,
) -> ArtifactOutputMetadataRecord {
    let backend_state = read_backend_state(&artifact_spec_state_path(artifact));
    ArtifactOutputMetadataRecord {
        artifact_id: artifact.id.as_str().to_string(),
        provider: format!("{:?}", artifact.provider_kind()),
        output_path: artifact.output.path.clone(),
        resolved_identifier_kind: backend_state.get("resolved_identifier_kind").cloned(),
        resolved_identifier: backend_state.get("resolved_identifier").cloned(),
        produced_filename: backend_state.get("produced_filename").cloned(),
        output_class: backend_state.get("output_class").cloned(),
        build_tool: backend_state.get("build_tool").cloned(),
        build_tool_version: backend_state.get("build_tool_version").cloned(),
    }
}

pub(crate) fn runtime_state_dir(spec: &ResolvedBuildSpec) -> PathBuf {
    PathBuf::from(&spec.workspace.out_dir)
        .join(".gaia")
        .join("runtime")
}

fn parse_backend_state(contents: &str) -> BTreeMap<String, String> {
    gaia_spec::KeyValueState::parse(contents).into_map()
}
