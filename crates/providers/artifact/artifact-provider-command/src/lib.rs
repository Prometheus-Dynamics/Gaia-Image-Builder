//! `kind = "command"`: runs a host program in its source's directory and
//! takes a file or a directory as the artifact's output.
//!
//! The program sees `GAIA_ARTIFACT_OUTPUT`, the absolute output path. With
//! `collect`, Gaia copies that path (relative to the source directory) to the
//! output after the program exits, leaving out `collect_exclude`; without it,
//! the program writes the output itself. A directory output keeps its Gaia
//! state inside its own `.gaia`, like a Rust directory artifact, and installs
//! the same way.

mod collect;

use gaia_artifact_providers::{
    ArtifactBackendState, ArtifactExecutionBackend, ArtifactExecutionContract,
    ArtifactOutputContract, ArtifactOutputKind, ArtifactProvider, ArtifactProviderError,
    ArtifactProviderErrorKind, ArtifactProviderValidationIssue, ProcessCancelCheck, ProcessLogSink,
    artifact_output_path, dir_digest, ensure_artifact_output_parent, render_artifact_backend_state,
    run_command_with_retries, write_artifact_marker,
};
use gaia_spec::{ArtifactDefinition, ArtifactSpec, CommandArtifactSpec, ResolvedBuildSpec};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The variable that carries the absolute output path to the command.
pub const OUTPUT_ENV: &str = "GAIA_ARTIFACT_OUTPUT";

pub struct CommandProvider;

impl ArtifactProvider for CommandProvider {
    fn id(&self) -> &'static str {
        "artifact.command"
    }

    fn kind(&self) -> gaia_spec::ArtifactProviderKind {
        gaia_spec::ArtifactProviderKind::Command
    }

    fn supports(&self, _spec: &ResolvedBuildSpec) -> bool {
        true
    }

    fn validate_artifact(&self, artifact: &ArtifactSpec) -> Vec<ArtifactProviderValidationIssue> {
        let mut issues = Vec::new();
        let ArtifactDefinition::Command(command) = &artifact.definition else {
            return issues;
        };
        if command.command.is_empty() {
            issues.push(ArtifactProviderValidationIssue {
                code: "command_artifact_command_empty",
                message: "command artifact command cannot be empty".into(),
            });
        }
        if let Some(target) = &artifact.target
            && !target.trim().is_empty()
        {
            issues.push(ArtifactProviderValidationIssue {
                code: "command_artifact_target_unsupported",
                message: format!(
                    "command artifact target '{target}' is not supported; a command artifact runs on the host"
                ),
            });
        }
        issues
    }

    fn execute_artifact(
        &self,
        artifact: &ArtifactSpec,
        contract: &ArtifactExecutionContract,
        log_sink: Option<ProcessLogSink>,
        cancel_check: Option<ProcessCancelCheck>,
    ) -> Result<Vec<String>, ArtifactProviderError> {
        let ArtifactDefinition::Command(spec) = &artifact.definition else {
            return Err(ArtifactProviderError::new(
                ArtifactProviderErrorKind::PolicyBlocked,
                format!(
                    "artifact '{}' is not a command artifact",
                    artifact.id.as_str()
                ),
            ));
        };
        if let ArtifactExecutionBackend::Docker(_) = contract.execution_backend {
            return Err(ArtifactProviderError::new(
                ArtifactProviderErrorKind::PolicyBlocked,
                format!(
                    "command artifact '{}' runs on the host only; docker execution is not supported",
                    artifact.id.as_str()
                ),
            ));
        }
        let source_dir = contract.source_dir.as_deref().unwrap_or(".");
        let output_path = absolute(&artifact_output_path(contract, source_dir));
        // A stale output must not pass for this run's.
        collect::remove_existing(&output_path)?;
        ensure_artifact_output_parent(&output_path)?;

        let command = build_command(artifact, spec, source_dir, &output_path)?;
        run_command_with_retries(
            &command,
            contract,
            &format!("command artifact '{}'", artifact.id.as_str()),
            log_sink,
            cancel_check,
        )?;

        if let Some(collect) = &spec.collect {
            let from = Path::new(source_dir).join(collect);
            if !from.exists() {
                return Err(ArtifactProviderError::new(
                    ArtifactProviderErrorKind::RuntimeState,
                    format!(
                        "command artifact '{}': collect path '{}' does not exist after the command",
                        artifact.id.as_str(),
                        from.display()
                    ),
                ));
            }
            collect::copy_collected(&from, &output_path, &spec.collect_exclude)?;
        }
        let Ok(metadata) = std::fs::metadata(&output_path) else {
            return Err(ArtifactProviderError::new(
                ArtifactProviderErrorKind::RuntimeState,
                format!(
                    "command artifact '{}' produced no output at '{}'{}",
                    artifact.id.as_str(),
                    output_path.display(),
                    if spec.collect.is_none() {
                        format!(" (the command must create ${OUTPUT_ENV})")
                    } else {
                        String::new()
                    }
                ),
            ));
        };
        let is_dir = metadata.is_dir();
        let contract = output_contract(contract, &output_path, is_dir);
        let program = spec.command.first().map(String::as_str).unwrap_or("");
        write_artifact_marker(
            &contract,
            self.id(),
            artifact.id.as_str(),
            &[("program", program)],
            &artifact_state_contents(self.id(), artifact.id.as_str(), &contract, spec, is_dir),
        )?;
        Ok(vec![format!(
            "command artifact '{}' ran '{}' -> {}{}",
            artifact.id.as_str(),
            program,
            output_path.display(),
            if is_dir { " (directory)" } else { "" }
        )])
    }
}

fn build_command(
    artifact: &ArtifactSpec,
    spec: &CommandArtifactSpec,
    source_dir: &str,
    output_path: &Path,
) -> Result<Command, ArtifactProviderError> {
    let Some((program, args)) = spec.command.split_first() else {
        return Err(ArtifactProviderError::new(
            ArtifactProviderErrorKind::PolicyBlocked,
            format!(
                "command artifact '{}' has an empty command",
                artifact.id.as_str()
            ),
        ));
    };
    let mut command = Command::new(program);
    command.args(args).current_dir(source_dir);
    // A `${secret:NAME}` in a value is read here, when the process starts,
    // and its value goes only to the process.
    for (key, value) in &spec.build_env {
        let value = gaia_process::secrets::expand_child_value(value).map_err(|error| {
            ArtifactProviderError::new(
                ArtifactProviderErrorKind::PolicyBlocked,
                format!("build_env '{key}': {error}"),
            )
        })?;
        command.env(key, value);
    }
    command.env(OUTPUT_ENV, output_path);
    Ok(command)
}

/// The contract with the output the command produced: the absolute path, and
/// a directory kind when it made a directory, so the marker and state go in
/// the tree's own `.gaia` as for every directory artifact.
fn output_contract(
    contract: &ArtifactExecutionContract,
    output_path: &Path,
    is_dir: bool,
) -> ArtifactExecutionContract {
    ArtifactExecutionContract {
        output: ArtifactOutputContract {
            path: output_path.display().to_string(),
            kind: if is_dir {
                ArtifactOutputKind::Directory
            } else {
                ArtifactOutputKind::File
            },
        },
        ..contract.clone()
    }
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn artifact_state_contents(
    provider_id: &str,
    artifact_id: &str,
    contract: &ArtifactExecutionContract,
    spec: &CommandArtifactSpec,
    is_dir: bool,
) -> String {
    let program = spec.command.first().map(String::as_str).unwrap_or("");
    let mut extra = vec![("command".to_string(), spec.command.join(" "))];
    if let Some(collect) = &spec.collect {
        extra.push(("collect".to_string(), collect.clone()));
    }
    if is_dir {
        extra.push((
            "output_dir_digest".to_string(),
            dir_digest(Path::new(&contract.output.path)),
        ));
    }
    render_artifact_backend_state(ArtifactBackendState {
        contract,
        provider_id,
        artifact_id,
        resolved_identifier_kind: "command",
        resolved_identifier: program,
        output_class: if is_dir {
            "command-directory"
        } else {
            "command-file"
        },
        build_tool: "command",
        // No version probe: the program is arbitrary, and its argv is in
        // the artifact's identity.
        build_tool_version: "unprobed",
        extra_fields: &extra,
    })
}

#[cfg(test)]
mod tests;
