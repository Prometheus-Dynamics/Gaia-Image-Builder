use gaia_artifact_providers::{
    ArtifactBackendState, ArtifactExecutionContract, ArtifactPlan, ArtifactProvider,
    ArtifactProviderError, ArtifactProviderErrorKind, ArtifactProviderOperation,
    ArtifactProviderValidationIssue, ProcessCancelCheck, ProcessLogSink, artifact_output_path,
    artifact_package_root, command_version_line, copy_artifact_file_to_output,
    materialize_artifact_marker_and_state, render_artifact_backend_state, run_command_with_retries,
};
use gaia_spec::{ArtifactDefinition, ArtifactSpec, ResolvedBuildSpec};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct PythonProvider;

impl ArtifactProvider for PythonProvider {
    fn id(&self) -> &'static str {
        "artifact.python"
    }

    fn kind(&self) -> gaia_spec::ArtifactProviderKind {
        gaia_spec::ArtifactProviderKind::Python
    }

    fn supports(&self, _spec: &ResolvedBuildSpec) -> bool {
        true
    }

    fn plan_artifact(&self, artifact: &ArtifactSpec) -> ArtifactPlan {
        ArtifactPlan {
            operations: vec![ArtifactProviderOperation::Build],
            contract: ArtifactExecutionContract::from_spec(
                artifact,
                None,
                false,
                ArtifactExecutionContract::default_command_policy(),
                gaia_spec::OutputRetentionPolicySpec::default(),
            ),
        }
    }

    fn validate_artifact(&self, artifact: &ArtifactSpec) -> Vec<ArtifactProviderValidationIssue> {
        let mut issues = Vec::new();
        if let ArtifactDefinition::Python(python) = &artifact.definition
            && python.package_dir.trim().is_empty()
        {
            issues.push(ArtifactProviderValidationIssue {
                code: "python_package_dir_empty",
                message: "python package_dir cannot be empty".into(),
            });
        }
        if let Some(target) = &artifact.target
            && !target.trim().is_empty()
        {
            issues.push(ArtifactProviderValidationIssue {
                code: "python_artifact_target_unsupported",
                message: format!(
                    "python artifact target '{}' is not supported; python artifacts are currently host-built only",
                    target
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
        reject_unsupported_artifact_target(artifact)?;
        let package_dir = match &artifact.definition {
            ArtifactDefinition::Python(python) => python.package_dir.clone(),
            _ => artifact.id.as_str().to_string(),
        };
        let source_dir = contract.source_dir.as_deref().unwrap_or(".");
        let package_root = artifact_package_root(source_dir, &package_dir);
        let wheelhouse = package_root.join(".gaia-wheelhouse");
        fs::create_dir_all(&wheelhouse).map_err(|error| {
            format!(
                "failed to create python wheelhouse '{}': {error}",
                wheelhouse.display()
            )
        })?;

        let mut command = Command::new("python3");
        command
            .arg("-m")
            .arg("pip")
            .arg("wheel")
            .arg(".")
            .arg("--no-deps")
            .arg("-w")
            .arg(&wheelhouse)
            .current_dir(&package_root);
        run_command(command, &package_dir, contract, log_sink, cancel_check)?;
        let mut messages = Vec::new();

        let built_wheel = find_built_wheel(&wheelhouse)?;
        let output_path = artifact_output_path(contract, source_dir);
        copy_artifact_file_to_output(&built_wheel, &output_path, "built python artifact")?;
        write_marker(self.id(), artifact, contract, &package_dir)?;
        messages.push(format!(
            "python artifact '{}' built package '{}' -> {}",
            artifact.id.as_str(),
            package_dir,
            contract.output.path
        ));
        Ok(messages)
    }
}

fn reject_unsupported_artifact_target(
    artifact: &ArtifactSpec,
) -> Result<(), ArtifactProviderError> {
    if let Some(target) = &artifact.target
        && !target.trim().is_empty()
    {
        return Err(ArtifactProviderError::new(
            ArtifactProviderErrorKind::PolicyBlocked,
            format!(
                "python artifact '{}' declared target '{}', but python target-aware builds are not supported yet",
                artifact.id.as_str(),
                target
            ),
        ));
    }
    Ok(())
}

fn find_built_wheel(wheelhouse: &Path) -> Result<PathBuf, ArtifactProviderError> {
    let mut wheels = fs::read_dir(wheelhouse)
        .map_err(|error| {
            ArtifactProviderError::new(
                ArtifactProviderErrorKind::RuntimeState,
                format!(
                    "failed to read python wheelhouse '{}': {error}",
                    wheelhouse.display()
                ),
            )
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("whl"))
        .collect::<Vec<_>>();
    wheels.sort();
    wheels.pop().ok_or_else(|| {
        ArtifactProviderError::new(
            ArtifactProviderErrorKind::OutputMissing,
            format!(
                "python build completed but no wheel file was produced in '{}'",
                wheelhouse.display()
            ),
        )
    })
}

fn run_command(
    command: Command,
    package_dir: &str,
    contract: &ArtifactExecutionContract,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Vec<String>, ArtifactProviderError> {
    run_command_with_retries(
        &command,
        contract,
        &format!("python wheel build for package dir '{package_dir}'"),
        log_sink,
        cancel_check,
    )?;
    Ok(Vec::new())
}

fn write_marker(
    provider_id: &str,
    artifact: &ArtifactSpec,
    contract: &ArtifactExecutionContract,
    package_dir: &str,
) -> Result<(), ArtifactProviderError> {
    materialize_artifact_marker_and_state(
        contract,
        &format!(
            "provider={provider_id}\nartifact={}\npackage_dir={package_dir}\n",
            artifact.id.as_str()
        ),
        &artifact_state_contents(provider_id, artifact.id.as_str(), contract, package_dir),
    )
}

fn artifact_state_contents(
    provider_id: &str,
    artifact_id: &str,
    contract: &ArtifactExecutionContract,
    package_dir: &str,
) -> String {
    render_artifact_backend_state(ArtifactBackendState {
        contract,
        provider_id,
        artifact_id,
        resolved_identifier_kind: "package-dir",
        resolved_identifier: package_dir,
        output_class: "wheel",
        build_tool: "pip-wheel",
        build_tool_version: &format!(
            "{} | {}",
            command_version_line("python3", &["--version"]),
            command_version_line("python3", &["-m", "pip", "--version"])
        ),
        extra_fields: &[("package_dir".to_string(), package_dir.to_string())],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(prefix: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir()
            .join("gaia-tests")
            .join(format!("{prefix}-{nonce}"))
    }

    #[test]
    fn run_command_reports_missing_tool() {
        let error = run_command(
            Command::new("gaia-missing-python-tool"),
            "package-dir",
            &ArtifactExecutionContract::from_spec(
                &ArtifactSpec::new(
                    "python-missing-tool",
                    ArtifactDefinition::Python(gaia_spec::PythonArtifactSpec {
                        package_dir: "package-dir".into(),
                    }),
                    None,
                    gaia_spec::ArtifactOutputSpec {
                        path: "out/package.whl".into(),
                    },
                ),
                None,
                false,
                ArtifactExecutionContract::default_command_policy(),
                gaia_spec::OutputRetentionPolicySpec::default(),
            ),
            None,
            None,
        )
        .expect_err("missing tool should fail");

        assert_eq!(
            error.kind,
            gaia_artifact_providers::ArtifactProviderErrorKind::ToolStart
        );
        assert!(
            error
                .message
                .contains("failed to start python wheel build for package dir")
        );
    }

    #[test]
    fn python_artifact_state_persists_backend_native_fields() {
        let output_path = temp_path("gaia-python-provider-state");
        fs::write(&output_path, "artifact").expect("output");
        let artifact = ArtifactSpec::new(
            "python-artifact",
            ArtifactDefinition::Python(gaia_spec::PythonArtifactSpec {
                package_dir: "packages/sdk".into(),
            }),
            None,
            gaia_spec::ArtifactOutputSpec {
                path: output_path.display().to_string(),
            },
        );
        let contract = ArtifactExecutionContract::from_spec(
            &artifact,
            None,
            false,
            ArtifactExecutionContract::default_command_policy(),
            gaia_spec::OutputRetentionPolicySpec::default(),
        );

        let state = artifact_state_contents(
            "artifact.python",
            artifact.id.as_str(),
            &contract,
            "packages/sdk",
        );

        assert!(state.contains("resolved_identifier_kind=package-dir"));
        assert!(state.contains("resolved_identifier=packages/sdk"));
        assert!(state.contains("output_class=wheel"));
        assert!(state.contains("build_tool=pip-wheel"));
        assert!(state.contains("package_dir=packages/sdk"));
    }

    #[test]
    fn validate_artifact_rejects_target_override() {
        let mut artifact = ArtifactSpec::new(
            "python-targeted",
            ArtifactDefinition::Python(gaia_spec::PythonArtifactSpec {
                package_dir: "packages/sdk".into(),
            }),
            None,
            gaia_spec::ArtifactOutputSpec {
                path: "out/package.whl".into(),
            },
        );
        artifact.target = Some("linux/arm64".into());

        let issues = PythonProvider.validate_artifact(&artifact);

        assert!(
            issues
                .iter()
                .any(|issue| issue.code == "python_artifact_target_unsupported")
        );
    }

    #[test]
    fn execute_artifact_rejects_target_override() {
        let mut artifact = ArtifactSpec::new(
            "python-targeted",
            ArtifactDefinition::Python(gaia_spec::PythonArtifactSpec {
                package_dir: "packages/sdk".into(),
            }),
            None,
            gaia_spec::ArtifactOutputSpec {
                path: "out/package.whl".into(),
            },
        );
        artifact.target = Some("linux/arm64".into());
        let contract = ArtifactExecutionContract::from_spec(
            &artifact,
            Some(temp_path("gaia-python-provider-src").display().to_string()),
            false,
            ArtifactExecutionContract::default_command_policy(),
            gaia_spec::OutputRetentionPolicySpec::default(),
        );

        let error = PythonProvider
            .execute_artifact(&artifact, &contract, None, None)
            .expect_err("targeted python artifact should fail");

        assert_eq!(error.kind, ArtifactProviderErrorKind::PolicyBlocked);
        assert!(
            error
                .message
                .contains("target-aware builds are not supported yet")
        );
    }
}
