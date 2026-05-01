use std::collections::HashSet;

use gaia_spec::{ArtifactDefinition, ArtifactInstallClassSpec, BuildModeSpec, ResolvedBuildSpec};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;

pub(crate) fn validate_artifacts(
    spec: &ResolvedBuildSpec,
    source_ids: &HashSet<String>,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) -> HashSet<String> {
    let mut artifact_ids = HashSet::new();
    let mut install_identities = HashSet::new();
    for artifact in &spec.artifacts {
        if !artifact.id.is_valid() {
            diagnostics.push(error(
                "artifact_id_empty",
                "artifact id cannot be empty".into(),
                Some("artifact".into()),
            ));
        }
        if !artifact_ids.insert(artifact.id.as_str().to_string()) {
            diagnostics.push(error(
                "duplicate_artifact_id",
                format!("duplicate artifact id '{}'", artifact.id.as_str()),
                Some(format!("artifact:{}", artifact.id.as_str())),
            ));
        }
        if let Some(source) = &artifact.source
            && !source_ids.contains(source.id.as_str())
        {
            diagnostics.push(error(
                "unknown_artifact_source",
                format!(
                    "artifact '{}' references unknown source '{}'",
                    artifact.id.as_str(),
                    source.id.as_str()
                ),
                Some(format!("artifact:{}", artifact.id.as_str())),
            ));
        }
        for dependency in &artifact.dependencies {
            if dependency.id.as_str() == artifact.id.as_str() {
                diagnostics.push(error(
                    "self_artifact_dependency",
                    format!("artifact '{}' depends on itself", artifact.id.as_str()),
                    Some(format!("artifact:{}", artifact.id.as_str())),
                ));
            }
        }
        if artifact.output.path.trim().is_empty() {
            diagnostics.push(error(
                "artifact_output_empty",
                format!(
                    "artifact '{}' has an empty output path",
                    artifact.id.as_str()
                ),
                Some(format!("artifact:{}", artifact.id.as_str())),
            ));
        }
        if let Some(target) = &artifact.target
            && target.trim().is_empty()
        {
            diagnostics.push(error(
                "artifact_target_empty",
                format!("artifact '{}' has an empty target", artifact.id.as_str()),
                Some(format!("artifact:{}", artifact.id.as_str())),
            ));
        }
        if let Some(identity) = &artifact.install_identity {
            if identity.install_name.trim().is_empty() {
                diagnostics.push(error(
                    "artifact_install_name_empty",
                    format!(
                        "artifact '{}' has an empty install-facing name",
                        artifact.id.as_str()
                    ),
                    Some(format!("artifact:{}", artifact.id.as_str())),
                ));
            }
            if let Some(destination_hint) = &identity.destination_hint
                && !destination_hint.starts_with('/')
            {
                diagnostics.push(error(
                    "artifact_install_dest_hint_not_absolute",
                    format!(
                        "artifact '{}' has non-absolute install destination hint '{}'",
                        artifact.id.as_str(),
                        destination_hint
                    ),
                    Some(format!("artifact:{}", artifact.id.as_str())),
                ));
            }
            let identity_key = (
                identity.install_name.clone(),
                artifact_install_class_name(identity.install_class).to_string(),
                identity.destination_hint.clone().unwrap_or_default(),
            );
            if !install_identities.insert(identity_key) {
                diagnostics.push(error(
                "duplicate_artifact_install_identity",
                format!(
                    "artifact '{}' duplicates an install-facing identity already claimed by another artifact",
                    artifact.id.as_str()
                ),
                Some(format!("artifact:{}", artifact.id.as_str())),
            ));
            }
        }
        match &artifact.definition {
            ArtifactDefinition::Rust(rust) => {
                if rust.package.trim().is_empty() {
                    diagnostics.push(error(
                        "rust_package_empty",
                        format!(
                            "rust artifact '{}' has an empty package",
                            artifact.id.as_str()
                        ),
                        Some(format!("artifact:{}", artifact.id.as_str())),
                    ));
                }
            }
            ArtifactDefinition::Java(java) => {
                if java.build_target.trim().is_empty() {
                    diagnostics.push(error(
                        "java_target_empty",
                        format!(
                            "java artifact '{}' has an empty build target",
                            artifact.id.as_str()
                        ),
                        Some(format!("artifact:{}", artifact.id.as_str())),
                    ));
                }
            }
            ArtifactDefinition::Node(node) => {
                if node.package_dir.trim().is_empty() {
                    diagnostics.push(error(
                        "node_package_dir_empty",
                        format!(
                            "node artifact '{}' has an empty package dir",
                            artifact.id.as_str()
                        ),
                        Some(format!("artifact:{}", artifact.id.as_str())),
                    ));
                }
            }
            ArtifactDefinition::Python(python) => {
                if python.package_dir.trim().is_empty() {
                    diagnostics.push(error(
                        "python_package_dir_empty",
                        format!(
                            "python artifact '{}' has an empty package dir",
                            artifact.id.as_str()
                        ),
                        Some(format!("artifact:{}", artifact.id.as_str())),
                    ));
                }
            }
            ArtifactDefinition::Go(go) => {
                if go.package.trim().is_empty() {
                    diagnostics.push(error(
                        "go_package_empty",
                        format!(
                            "go artifact '{}' has an empty package",
                            artifact.id.as_str()
                        ),
                        Some(format!("artifact:{}", artifact.id.as_str())),
                    ));
                }
            }
        }
        if let Some(BuildModeSpec::Custom(mode)) = &artifact.build_mode
            && mode.trim().is_empty()
        {
            diagnostics.push(error(
                "artifact_build_mode_empty",
                format!(
                    "artifact '{}' has an empty custom build mode",
                    artifact.id.as_str()
                ),
                Some(format!("artifact:{}", artifact.id.as_str())),
            ));
        }
    }
    diagnostics.extend(validate_artifact_cycles(spec));
    artifact_ids
}

fn artifact_install_class_name(class: ArtifactInstallClassSpec) -> &'static str {
    class.as_str()
}

fn validate_artifact_cycles(spec: &ResolvedBuildSpec) -> Vec<ValidationDiagnostic> {
    let mut diagnostics = Vec::new();
    let mut permanent = HashSet::new();
    let mut visiting = Vec::<String>::new();

    for artifact in &spec.artifacts {
        detect_cycle(
            spec,
            artifact.id.as_str(),
            &mut permanent,
            &mut visiting,
            &mut diagnostics,
        );
    }

    diagnostics
}

fn detect_cycle(
    spec: &ResolvedBuildSpec,
    artifact_id: &str,
    permanent: &mut HashSet<String>,
    visiting: &mut Vec<String>,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    if permanent.contains(artifact_id) {
        return;
    }

    if let Some(index) = visiting.iter().position(|id| id == artifact_id) {
        let mut cycle = visiting[index..].to_vec();
        cycle.push(artifact_id.to_string());
        diagnostics.push(error(
            "artifact_dependency_cycle",
            format!("artifact dependency cycle detected: {}", cycle.join(" -> ")),
            Some(format!("artifact:{}", artifact_id)),
        ));
        return;
    }

    let Some(artifact) = spec
        .artifacts
        .iter()
        .find(|artifact| artifact.id.as_str() == artifact_id)
    else {
        return;
    };

    visiting.push(artifact_id.to_string());
    for dependency in &artifact.dependencies {
        if spec
            .artifacts
            .iter()
            .any(|candidate| candidate.id.as_str() == dependency.id.as_str())
        {
            detect_cycle(
                spec,
                dependency.id.as_str(),
                permanent,
                visiting,
                diagnostics,
            );
        }
    }
    visiting.pop();
    permanent.insert(artifact_id.to_string());
}
