use crate::{
    ArtifactExecutionBackend, ArtifactExecutionContract, ArtifactOutputKind, ArtifactProviderError,
    ArtifactProviderErrorKind,
};
use std::fs;
use std::path::{Path, PathBuf};

pub fn materialize_artifact_output(
    contract: &ArtifactExecutionContract,
    contents: &str,
) -> Result<(), ArtifactProviderError> {
    let output_path = Path::new(&contract.output.path);
    match contract.output.kind {
        ArtifactOutputKind::File => {
            if let Some(parent) = output_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| {
                        format!(
                            "failed to create artifact output dir '{}': {error}",
                            parent.display()
                        )
                    })
                    .map_err(ArtifactProviderError::backend_command)?;
            }
            let temp_output = output_path.with_extension("gaia.tmp");
            fs::write(&temp_output, contents)
                .map_err(|error| {
                    format!(
                        "failed to write artifact temp output '{}': {error}",
                        temp_output.display()
                    )
                })
                .map_err(ArtifactProviderError::backend_command)?;
            finalize_temp_output(&temp_output, output_path, "artifact output")?;
        }
        ArtifactOutputKind::Directory => {
            fs::create_dir_all(output_path)
                .map_err(|error| {
                    format!(
                        "failed to create artifact output dir '{}': {error}",
                        output_path.display()
                    )
                })
                .map_err(ArtifactProviderError::backend_command)?;
            let marker = output_path.join(".gaia-artifact.txt");
            fs::write(&marker, contents)
                .map_err(|error| {
                    format!(
                        "failed to write artifact marker '{}': {error}",
                        marker.display()
                    )
                })
                .map_err(ArtifactProviderError::backend_command)?;
        }
    }
    Ok(())
}

pub fn finalize_temp_output(
    temp_output: &Path,
    output_path: &Path,
    label: &str,
) -> Result<(), ArtifactProviderError> {
    fs::rename(temp_output, output_path).map_err(|error| {
        let _ = fs::remove_file(temp_output);
        ArtifactProviderError::new(
            ArtifactProviderErrorKind::BackendCommand,
            format!(
                "failed to move {label} '{}' into place '{}': {error}",
                temp_output.display(),
                output_path.display()
            ),
        )
    })
}

pub fn artifact_output_path(contract: &ArtifactExecutionContract, source_dir: &str) -> PathBuf {
    let path = PathBuf::from(&contract.output.path);
    if path.is_absolute() {
        path
    } else if let Some(workspace_root) = &contract.workspace_root {
        PathBuf::from(workspace_root).join(path)
    } else {
        PathBuf::from(source_dir).join(path)
    }
}

pub fn artifact_package_root(source_dir: &str, package_dir: &str) -> PathBuf {
    let package_dir = PathBuf::from(package_dir);
    if package_dir.is_absolute() {
        package_dir
    } else {
        PathBuf::from(source_dir).join(package_dir)
    }
}

pub fn ensure_artifact_output_parent(output_path: &Path) -> Result<(), ArtifactProviderError> {
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| {
                format!(
                    "failed to create artifact output dir '{}': {error}",
                    parent.display()
                )
            })
            .map_err(ArtifactProviderError::runtime_state)?;
    }
    Ok(())
}

pub fn copy_artifact_file_to_output(
    built_path: &Path,
    output_path: &Path,
    label: &str,
) -> Result<(), ArtifactProviderError> {
    ensure_artifact_output_parent(output_path)?;
    let temp_output = output_path.with_extension("gaia.copy.tmp");
    fs::copy(built_path, &temp_output)
        .map_err(|error| {
            format!(
                "failed to copy {label} '{}' to '{}': {error}",
                built_path.display(),
                temp_output.display()
            )
        })
        .map_err(ArtifactProviderError::runtime_state)?;
    finalize_temp_output(&temp_output, output_path, label)
}

pub fn artifact_marker_contract(contract: &ArtifactExecutionContract) -> ArtifactExecutionContract {
    ArtifactExecutionContract {
        output: crate::ArtifactOutputContract {
            path: artifact_sidecar_path(contract, "gaia-build.txt")
                .display()
                .to_string(),
            kind: crate::ArtifactOutputKind::File,
        },
        ..contract.clone()
    }
}

pub fn materialize_artifact_marker_and_state(
    contract: &ArtifactExecutionContract,
    marker_contents: &str,
    state_contents: &str,
) -> Result<(), ArtifactProviderError> {
    materialize_artifact_output(&artifact_marker_contract(contract), marker_contents)?;
    materialize_artifact_state(contract, state_contents)
}

pub fn artifact_state_path(contract: &ArtifactExecutionContract) -> PathBuf {
    artifact_sidecar_path(contract, "gaia-state.txt")
}

pub fn artifact_sidecar_path(contract: &ArtifactExecutionContract, suffix: &str) -> PathBuf {
    let output_path = Path::new(&contract.output.path);
    match contract.output.kind {
        ArtifactOutputKind::File => output_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(".gaia")
            .join(format!(
                "{}.{suffix}",
                output_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("artifact")
            )),
        ArtifactOutputKind::Directory => {
            output_path.join(".gaia").join(format!("artifact.{suffix}"))
        }
    }
}

pub fn materialize_artifact_state(
    contract: &ArtifactExecutionContract,
    contents: &str,
) -> Result<(), ArtifactProviderError> {
    let state_path = artifact_state_path(contract);
    if let Some(parent) = state_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| {
                format!(
                    "failed to create artifact state dir '{}': {error}",
                    parent.display()
                )
            })
            .map_err(ArtifactProviderError::runtime_state)?;
    }
    fs::write(&state_path, contents)
        .map_err(|error| {
            format!(
                "failed to write artifact state '{}': {error}",
                state_path.display()
            )
        })
        .map_err(ArtifactProviderError::runtime_state)
}

pub fn render_build_context_state(contract: &ArtifactExecutionContract) -> String {
    let (execution_backend, execution_backend_image) = match &contract.execution_backend {
        ArtifactExecutionBackend::Host => ("host", ""),
        ArtifactExecutionBackend::Docker(docker) => ("docker", docker.image.as_str()),
    };
    gaia_spec::KeyValueState::new()
        .with(
            "artifact_target",
            contract.artifact_target.as_deref().unwrap_or_default(),
        )
        .with("execution_backend", execution_backend)
        .with("execution_backend_image", execution_backend_image)
        .with(
            "build_version",
            contract.build_version.as_deref().unwrap_or_default(),
        )
        .with(
            "build_branch",
            contract.build_branch.as_deref().unwrap_or_default(),
        )
        .with(
            "build_target",
            contract.build_target.as_deref().unwrap_or_default(),
        )
        .with(
            "build_profile",
            contract.build_profile.as_deref().unwrap_or_default(),
        )
        .render()
}
