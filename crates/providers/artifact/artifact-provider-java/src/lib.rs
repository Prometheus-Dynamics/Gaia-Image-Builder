use gaia_artifact_providers::{
    ArtifactBackendState, ArtifactExecutionContract, ArtifactPlan, ArtifactProvider,
    ArtifactProviderError, ArtifactProviderErrorKind, ArtifactProviderOperation,
    ArtifactProviderValidationIssue, ProcessCancelCheck, ProcessLogSink, artifact_output_path,
    command_version_line, copy_artifact_file_to_output, materialize_artifact_marker_and_state,
    render_artifact_backend_state, run_command_with_retries,
};
use gaia_spec::{ArtifactDefinition, ArtifactSpec, ResolvedBuildSpec};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct JavaProvider;

impl ArtifactProvider for JavaProvider {
    fn id(&self) -> &'static str {
        "artifact.java"
    }

    fn kind(&self) -> gaia_spec::ArtifactProviderKind {
        gaia_spec::ArtifactProviderKind::Java
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
        if let ArtifactDefinition::Java(java) = &artifact.definition
            && java.build_target.trim().is_empty()
        {
            issues.push(ArtifactProviderValidationIssue {
                code: "java_build_target_empty",
                message: "java build_target cannot be empty".into(),
            });
        }
        if let Some(target) = &artifact.target
            && !target.trim().is_empty()
        {
            issues.push(ArtifactProviderValidationIssue {
                code: "java_artifact_target_unsupported",
                message: format!(
                    "java artifact target '{}' is not supported; java artifacts are currently host-built only",
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
        let build_target = match &artifact.definition {
            ArtifactDefinition::Java(java) => java.build_target.clone(),
            _ => artifact.id.as_str().to_string(),
        };
        let source_dir = contract.source_dir.as_deref().unwrap_or(".");
        let (build_tool, mut messages) =
            run_java_build(source_dir, contract, log_sink, cancel_check)?;
        let built_path = resolve_java_built_path(source_dir, &build_target)?;
        let output_path = artifact_output_path(contract, source_dir);
        copy_artifact_file_to_output(&built_path, &output_path, "built java artifact")?;
        write_marker(self.id(), artifact, contract, &build_target, &build_tool)?;
        messages.push(format!(
            "java artifact '{}' built target '{}' -> {}",
            artifact.id.as_str(),
            build_target,
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
                "java artifact '{}' declared target '{}', but java target-aware builds are not supported yet",
                artifact.id.as_str(),
                target
            ),
        ));
    }
    Ok(())
}

fn run_java_build(
    source_dir: &str,
    contract: &ArtifactExecutionContract,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(String, Vec<String>), ArtifactProviderError> {
    let source_dir = Path::new(source_dir);
    if source_dir.join("pom.xml").is_file() {
        let mut command = Command::new("mvn");
        command
            .arg("-q")
            .arg("-DskipTests")
            .arg("package")
            .current_dir(source_dir);
        return Ok((
            "maven".to_string(),
            run_command(
                command,
                "maven package",
                contract,
                log_sink,
                cancel_check.clone(),
            )?,
        ));
    }

    if source_dir.join("gradlew").is_file() {
        let mut command = Command::new(source_dir.join("gradlew"));
        command.arg("build").arg("-q").current_dir(source_dir);
        return Ok((
            "gradle-wrapper".to_string(),
            run_command(
                command,
                "gradle wrapper build",
                contract,
                log_sink,
                cancel_check.clone(),
            )?,
        ));
    }

    if source_dir.join("build.gradle").is_file() || source_dir.join("build.gradle.kts").is_file() {
        let mut command = Command::new("gradle");
        command.arg("build").arg("-q").current_dir(source_dir);
        return Ok((
            "gradle".to_string(),
            run_command(
                command,
                "gradle build",
                contract,
                log_sink,
                cancel_check.clone(),
            )?,
        ));
    }

    Err(ArtifactProviderError::new(
        ArtifactProviderErrorKind::PolicyBlocked,
        format!(
            "java source '{}' did not contain a supported build file (pom.xml, gradlew, build.gradle, build.gradle.kts)",
            source_dir.display()
        ),
    ))
}

fn resolve_java_built_path(
    source_dir: &str,
    build_target: &str,
) -> Result<PathBuf, ArtifactProviderError> {
    let target = PathBuf::from(build_target);
    let candidates = [
        if target.is_absolute() {
            target.clone()
        } else {
            PathBuf::from(source_dir).join(&target)
        },
        PathBuf::from(source_dir).join("target").join(build_target),
        PathBuf::from(source_dir)
            .join("build")
            .join("libs")
            .join(build_target),
    ];

    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| ArtifactProviderError::new(
                ArtifactProviderErrorKind::OutputMissing,
                format!(
                "java build completed but built target '{}' was not found in expected locations",
                build_target
            )))
}

fn run_command(
    command: Command,
    label: &str,
    contract: &ArtifactExecutionContract,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Vec<String>, ArtifactProviderError> {
    run_command_with_retries(&command, contract, label, log_sink, cancel_check)?;
    Ok(Vec::new())
}

fn write_marker(
    provider_id: &str,
    artifact: &ArtifactSpec,
    contract: &ArtifactExecutionContract,
    build_target: &str,
    build_tool: &str,
) -> Result<(), ArtifactProviderError> {
    materialize_artifact_marker_and_state(
        contract,
        &format!(
            "provider={provider_id}\nartifact={}\nbuild_target={build_target}\n",
            artifact.id.as_str()
        ),
        &artifact_state_contents(
            provider_id,
            artifact.id.as_str(),
            contract,
            build_target,
            build_tool,
        ),
    )
}

fn artifact_state_contents(
    provider_id: &str,
    artifact_id: &str,
    contract: &ArtifactExecutionContract,
    build_target: &str,
    build_tool: &str,
) -> String {
    let build_tool_version = match build_tool {
        "maven" => command_version_line("mvn", &["-version"]),
        "gradle-wrapper" => {
            let source_dir = contract.source_dir.as_deref().unwrap_or(".");
            command_version_line(Path::new(source_dir).join("gradlew"), &["--version"])
        }
        _ => command_version_line("gradle", &["--version"]),
    };
    render_artifact_backend_state(ArtifactBackendState {
        contract,
        provider_id,
        artifact_id,
        resolved_identifier_kind: "build-target",
        resolved_identifier: build_target,
        output_class: "jar",
        build_tool,
        build_tool_version: &build_tool_version,
        extra_fields: &[("build_target".to_string(), build_target.to_string())],
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
            Command::new("gaia-missing-java-tool"),
            "java build",
            &ArtifactExecutionContract::from_spec(
                &ArtifactSpec::new(
                    "java-missing-tool",
                    ArtifactDefinition::Java(gaia_spec::JavaArtifactSpec {
                        build_target: "app.jar".into(),
                    }),
                    None,
                    gaia_spec::ArtifactOutputSpec {
                        path: "out/app.jar".into(),
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
        assert!(error.message.contains("failed to start java build"));
    }

    #[test]
    fn java_artifact_state_persists_backend_native_fields() {
        let output_path = temp_path("gaia-java-provider-state");
        fs::write(&output_path, "artifact").expect("output");
        let artifact = ArtifactSpec::new(
            "java-artifact",
            ArtifactDefinition::Java(gaia_spec::JavaArtifactSpec {
                build_target: "build/libs/app.jar".into(),
            }),
            None,
            gaia_spec::ArtifactOutputSpec {
                path: output_path.display().to_string(),
            },
        );
        let contract = ArtifactExecutionContract::from_spec(
            &artifact,
            Some(temp_path("gaia-java-provider-src").display().to_string()),
            false,
            ArtifactExecutionContract::default_command_policy(),
            gaia_spec::OutputRetentionPolicySpec::default(),
        );

        let state = artifact_state_contents(
            "artifact.java",
            artifact.id.as_str(),
            &contract,
            "build/libs/app.jar",
            "maven",
        );

        assert!(state.contains("resolved_identifier_kind=build-target"));
        assert!(state.contains("resolved_identifier=build/libs/app.jar"));
        assert!(state.contains("output_class=jar"));
        assert!(state.contains("build_tool=maven"));
        assert!(state.contains("build_target=build/libs/app.jar"));
    }

    #[test]
    fn validate_artifact_rejects_target_override() {
        let mut artifact = ArtifactSpec::new(
            "java-targeted",
            ArtifactDefinition::Java(gaia_spec::JavaArtifactSpec {
                build_target: "build/libs/app.jar".into(),
            }),
            None,
            gaia_spec::ArtifactOutputSpec {
                path: "out/app.jar".into(),
            },
        );
        artifact.target = Some("aarch64-unknown-linux-gnu".into());

        let issues = JavaProvider.validate_artifact(&artifact);

        assert!(
            issues
                .iter()
                .any(|issue| issue.code == "java_artifact_target_unsupported")
        );
    }

    #[test]
    fn execute_artifact_rejects_target_override() {
        let mut artifact = ArtifactSpec::new(
            "java-targeted",
            ArtifactDefinition::Java(gaia_spec::JavaArtifactSpec {
                build_target: "build/libs/app.jar".into(),
            }),
            None,
            gaia_spec::ArtifactOutputSpec {
                path: "out/app.jar".into(),
            },
        );
        artifact.target = Some("aarch64-unknown-linux-gnu".into());
        let contract = ArtifactExecutionContract::from_spec(
            &artifact,
            Some(temp_path("gaia-java-provider-src").display().to_string()),
            false,
            ArtifactExecutionContract::default_command_policy(),
            gaia_spec::OutputRetentionPolicySpec::default(),
        );

        let error = JavaProvider
            .execute_artifact(&artifact, &contract, None, None)
            .expect_err("targeted java artifact should fail");

        assert_eq!(error.kind, ArtifactProviderErrorKind::PolicyBlocked);
        assert!(
            error
                .message
                .contains("target-aware builds are not supported yet")
        );
    }
}
