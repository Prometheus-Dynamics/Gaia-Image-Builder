use gaia_artifact_providers::{
    ArtifactBackendState, ArtifactExecutionContract, ArtifactPlan, ArtifactProvider,
    ArtifactProviderError, ArtifactProviderErrorKind, ArtifactProviderOperation,
    ArtifactProviderValidationIssue, ProcessCancelCheck, ProcessLogSink, artifact_output_path,
    command_version_line, ensure_artifact_output_parent, materialize_artifact_marker_and_state,
    render_artifact_backend_state, run_command_with_retries,
};
use gaia_spec::{ArtifactDefinition, ArtifactSpec, ResolvedBuildSpec};
use std::process::Command;

pub struct GoProvider;

impl ArtifactProvider for GoProvider {
    fn id(&self) -> &'static str {
        "artifact.go"
    }

    fn kind(&self) -> gaia_spec::ArtifactProviderKind {
        gaia_spec::ArtifactProviderKind::Go
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
        if let ArtifactDefinition::Go(go) = &artifact.definition
            && go.package.trim().is_empty()
        {
            issues.push(ArtifactProviderValidationIssue {
                code: "go_package_empty",
                message: "go package cannot be empty".into(),
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
        let package = match &artifact.definition {
            ArtifactDefinition::Go(go) => go.package.clone(),
            _ => artifact.id.as_str().to_string(),
        };
        let source_dir = contract.source_dir.as_deref().unwrap_or(".");
        let output_path = artifact_output_path(contract, source_dir);
        ensure_artifact_output_parent(&output_path)?;

        let mut command = Command::new("go");
        command
            .arg("build")
            .arg("-o")
            .arg(&output_path)
            .arg(&package)
            .current_dir(source_dir);
        apply_go_target_env(&mut command, contract.artifact_target.as_deref())?;
        run_command(command, &package, contract, log_sink, cancel_check)?;
        let mut messages = Vec::new();
        write_marker(self.id(), artifact, contract, &package)?;
        messages.push(format!(
            "go artifact '{}' built package '{}' -> {}",
            artifact.id.as_str(),
            package,
            contract.output.path
        ));
        Ok(messages)
    }
}

fn run_command(
    command: Command,
    package: &str,
    contract: &ArtifactExecutionContract,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Vec<String>, ArtifactProviderError> {
    run_command_with_retries(
        &command,
        contract,
        &format!("go build for package '{package}'"),
        log_sink,
        cancel_check,
    )?;
    Ok(Vec::new())
}

fn apply_go_target_env(
    command: &mut Command,
    artifact_target: Option<&str>,
) -> Result<(), ArtifactProviderError> {
    let Some(artifact_target) = artifact_target else {
        return Ok(());
    };
    let parts = artifact_target.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        [goos, goarch] => {
            command.env("GOOS", goos);
            command.env("GOARCH", goarch);
            Ok(())
        }
        [goos, goarch, goarm] => {
            command.env("GOOS", goos);
            command.env("GOARCH", goarch);
            command.env("GOARM", goarm);
            Ok(())
        }
        _ => Err(ArtifactProviderError::new(
            ArtifactProviderErrorKind::RuntimeState,
            format!(
                "go artifact target '{artifact_target}' must be 'GOOS/GOARCH' or 'GOOS/GOARCH/GOARM'"
            ),
        )),
    }
}

fn write_marker(
    provider_id: &str,
    artifact: &ArtifactSpec,
    contract: &ArtifactExecutionContract,
    package: &str,
) -> Result<(), ArtifactProviderError> {
    materialize_artifact_marker_and_state(
        contract,
        &format!(
            "provider={provider_id}\nartifact={}\npackage={package}\n",
            artifact.id.as_str()
        ),
        &artifact_state_contents(provider_id, artifact.id.as_str(), contract, package),
    )
}

fn artifact_state_contents(
    provider_id: &str,
    artifact_id: &str,
    contract: &ArtifactExecutionContract,
    package: &str,
) -> String {
    render_artifact_backend_state(ArtifactBackendState {
        contract,
        provider_id,
        artifact_id,
        resolved_identifier_kind: "package",
        resolved_identifier: package,
        output_class: "binary",
        build_tool: "go",
        build_tool_version: &command_version_line("go", &["version"]),
        extra_fields: &[
            ("package".to_string(), package.to_string()),
            (
                "artifact_target".to_string(),
                contract.artifact_target.clone().unwrap_or_default(),
            ),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
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
            Command::new("gaia-missing-go-tool"),
            "example.com/pkg",
            &ArtifactExecutionContract::from_spec(
                &ArtifactSpec::new(
                    "go-missing-tool",
                    ArtifactDefinition::Go(gaia_spec::GoArtifactSpec {
                        package: "example.com/pkg".into(),
                    }),
                    None,
                    gaia_spec::ArtifactOutputSpec {
                        path: "out/bin".into(),
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
                .contains("failed to start go build for package")
        );
    }

    #[test]
    fn go_artifact_state_persists_backend_native_fields() {
        let output_path = temp_path("gaia-go-provider-state");
        fs::write(&output_path, "artifact").expect("output");
        let mut artifact = ArtifactSpec::new(
            "go-artifact",
            ArtifactDefinition::Go(gaia_spec::GoArtifactSpec {
                package: "example.com/service".into(),
            }),
            None,
            gaia_spec::ArtifactOutputSpec {
                path: output_path.display().to_string(),
            },
        );
        artifact.target = Some("linux/arm64".into());
        let contract = ArtifactExecutionContract::from_spec(
            &artifact,
            None,
            false,
            ArtifactExecutionContract::default_command_policy(),
            gaia_spec::OutputRetentionPolicySpec::default(),
        );

        let state = artifact_state_contents(
            "artifact.go",
            artifact.id.as_str(),
            &contract,
            "example.com/service",
        );

        assert!(state.contains("resolved_identifier_kind=package"));
        assert!(state.contains("resolved_identifier=example.com/service"));
        assert!(state.contains("output_class=binary"));
        assert!(state.contains("build_tool=go"));
        assert!(state.contains("package=example.com/service"));
        assert!(state.contains("artifact_target=linux/arm64"));
    }

    #[test]
    fn go_target_parser_rejects_invalid_target_shape() {
        let mut command = Command::new("go");
        let error = apply_go_target_env(&mut command, Some("not-a-valid-target"))
            .expect_err("invalid target shape should fail");

        assert_eq!(error.kind, ArtifactProviderErrorKind::RuntimeState);
        assert!(error.message.contains("GOOS/GOARCH"));
    }
}
