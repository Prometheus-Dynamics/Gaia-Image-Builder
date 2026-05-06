use gaia_spec::{ImageDefinition, ResolvedBuildSpec};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::model::{ArtifactOutputMetadataRecord, OutputHygieneWarningRecord};

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
        output_path.join(".gaia").join("artifact.gaia-state.txt")
    } else {
        output_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(".gaia")
            .join(format!(
                "{}.gaia-state.txt",
                output_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("artifact")
            ))
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
    PathBuf::from(&spec.workspace.out_dir).join(gaia_spec::RUNTIME_STATE_DIR_NAME)
}

pub(crate) fn output_hygiene_warnings(spec: &ResolvedBuildSpec) -> Vec<OutputHygieneWarningRecord> {
    let mut warnings = Vec::new();
    let Some(collect_dir) = &spec.image.output.collect_dir else {
        return warnings;
    };
    let collect_dir = PathBuf::from(collect_dir);
    if !collect_dir.is_dir() {
        return warnings;
    }
    let expected_names = expected_publish_filenames(spec);
    let entries = match fs::read_dir(&collect_dir) {
        Ok(entries) => entries,
        Err(_) => return warnings,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if spec
            .reporting
            .output_hygiene
            .transient_dir_names
            .iter()
            .any(|configured| configured == &name)
            && path.is_dir()
        {
            warnings.push(OutputHygieneWarningRecord {
                code: "publish_transient_directory".into(),
                directory: collect_dir.display().to_string(),
                path: path.display().to_string(),
                message: format!(
                    "publish directory contains transient directory '{}'",
                    path.display()
                ),
                size_bytes: None,
            });
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_file()
            && metadata.len() >= spec.reporting.output_hygiene.large_file_threshold_bytes
            && !expected_names.contains(&name)
            && !is_known_state_file(&name)
        {
            warnings.push(OutputHygieneWarningRecord {
                code: "publish_large_unexpected_file".into(),
                directory: collect_dir.display().to_string(),
                path: path.display().to_string(),
                message: format!(
                    "publish directory contains large non-output file '{}'",
                    path.display()
                ),
                size_bytes: Some(metadata.len()),
            });
        }
    }
    warnings
}

fn expected_publish_filenames(spec: &ResolvedBuildSpec) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    if let Some(archive_name) = &spec.image.output.archive_name {
        names.insert(archive_name.clone());
    }
    if let ImageDefinition::Buildroot(buildroot) = &spec.image.definition {
        for expected in &buildroot.expected_images {
            names.insert(expected.name.clone());
        }
    }
    if let Some(assembly) = &spec.image.assembly {
        for output in assembly
            .filesystems
            .iter()
            .map(|filesystem| filesystem.output.as_str())
            .chain(assembly.disks.iter().map(|disk| disk.output.as_str()))
        {
            if let Some(name) = Path::new(output).file_name().and_then(|name| name.to_str()) {
                names.insert(name.to_string());
            }
        }
    }
    names
}

fn is_known_state_file(name: &str) -> bool {
    name == ".gaia-image-state.txt" || name.ends_with(".gaia-state.txt")
}

fn parse_backend_state(contents: &str) -> BTreeMap<String, String> {
    gaia_spec::KeyValueState::parse(contents).into_map()
}
