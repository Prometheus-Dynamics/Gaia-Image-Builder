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

pub struct NodeProvider;

impl ArtifactProvider for NodeProvider {
    fn id(&self) -> &'static str {
        "artifact.node"
    }

    fn kind(&self) -> gaia_spec::ArtifactProviderKind {
        gaia_spec::ArtifactProviderKind::Node
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
        if let ArtifactDefinition::Node(node) = &artifact.definition
            && node.package_dir.trim().is_empty()
        {
            issues.push(ArtifactProviderValidationIssue {
                code: "node_package_dir_empty",
                message: "node package_dir cannot be empty".into(),
            });
        }
        if let Some(target) = &artifact.target
            && !target.trim().is_empty()
        {
            issues.push(ArtifactProviderValidationIssue {
                code: "node_artifact_target_unsupported",
                message: format!(
                    "node artifact target '{}' is not supported; node artifacts are currently host-built/packed only",
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
            ArtifactDefinition::Node(node) => node.package_dir.clone(),
            _ => artifact.id.as_str().to_string(),
        };
        let source_dir = contract.source_dir.as_deref().unwrap_or(".");
        let package_root = artifact_package_root(source_dir, &package_dir);
        let pack_dir = package_root.join(".gaia-pack");
        fs::create_dir_all(&pack_dir).map_err(|error| {
            format!(
                "failed to create node pack dir '{}': {error}",
                pack_dir.display()
            )
        })?;

        let mut command = Command::new("npm");
        command
            .arg("pack")
            .arg("--pack-destination")
            .arg(&pack_dir)
            .current_dir(&package_root);
        run_command(command, &package_dir, contract, log_sink, cancel_check)?;
        let mut messages = Vec::new();

        let tarball = find_packed_tarball(&pack_dir)?;
        let output_path = artifact_output_path(contract, source_dir);
        copy_artifact_file_to_output(&tarball, &output_path, "packed node artifact")?;
        write_marker(self.id(), artifact, contract, &package_dir)?;
        messages.push(format!(
            "node artifact '{}' built package '{}' -> {}",
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
                "node artifact '{}' declared target '{}', but node target-aware builds are not supported yet",
                artifact.id.as_str(),
                target
            ),
        ));
    }
    Ok(())
}

fn find_packed_tarball(pack_dir: &Path) -> Result<PathBuf, ArtifactProviderError> {
    let mut tarballs = fs::read_dir(pack_dir)
        .map_err(|error| {
            ArtifactProviderError::new(
                ArtifactProviderErrorKind::RuntimeState,
                format!(
                    "failed to read node pack dir '{}': {error}",
                    pack_dir.display()
                ),
            )
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".tgz"))
        })
        .collect::<Vec<_>>();
    tarballs.sort();
    tarballs.pop().ok_or_else(|| {
        ArtifactProviderError::new(
            ArtifactProviderErrorKind::OutputMissing,
            format!(
                "node pack completed but no tarball was produced in '{}'",
                pack_dir.display()
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
        &format!("npm pack for package dir '{package_dir}'"),
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
        output_class: "npm-tarball",
        build_tool: "npm-pack",
        build_tool_version: &command_version_line("npm", &["--version"]),
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
            Command::new("gaia-missing-node-tool"),
            "package-dir",
            &ArtifactExecutionContract::from_spec(
                &ArtifactSpec::new(
                    "node-missing-tool",
                    ArtifactDefinition::Node(gaia_spec::NodeArtifactSpec {
                        package_dir: "package-dir".into(),
                    }),
                    None,
                    gaia_spec::ArtifactOutputSpec {
                        path: "out/package.tgz".into(),
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
                .contains("failed to start npm pack for package dir")
        );
    }

    #[test]
    fn node_artifact_state_persists_backend_native_fields() {
        let output_path = temp_path("gaia-node-provider-state");
        fs::write(&output_path, "artifact").expect("output");
        let artifact = ArtifactSpec::new(
            "node-artifact",
            ArtifactDefinition::Node(gaia_spec::NodeArtifactSpec {
                package_dir: "packages/app".into(),
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
            "artifact.node",
            artifact.id.as_str(),
            &contract,
            "packages/app",
        );

        assert!(state.contains("resolved_identifier_kind=package-dir"));
        assert!(state.contains("resolved_identifier=packages/app"));
        assert!(state.contains("output_class=npm-tarball"));
        assert!(state.contains("build_tool=npm-pack"));
        assert!(state.contains("package_dir=packages/app"));
    }

    #[test]
    fn validate_artifact_rejects_target_override() {
        let mut artifact = ArtifactSpec::new(
            "node-targeted",
            ArtifactDefinition::Node(gaia_spec::NodeArtifactSpec {
                package_dir: "packages/app".into(),
            }),
            None,
            gaia_spec::ArtifactOutputSpec {
                path: "out/package.tgz".into(),
            },
        );
        artifact.target = Some("linux/arm64".into());

        let issues = NodeProvider.validate_artifact(&artifact);

        assert!(
            issues
                .iter()
                .any(|issue| issue.code == "node_artifact_target_unsupported")
        );
    }

    #[test]
    fn execute_artifact_rejects_target_override() {
        let mut artifact = ArtifactSpec::new(
            "node-targeted",
            ArtifactDefinition::Node(gaia_spec::NodeArtifactSpec {
                package_dir: "packages/app".into(),
            }),
            None,
            gaia_spec::ArtifactOutputSpec {
                path: "out/package.tgz".into(),
            },
        );
        artifact.target = Some("linux/arm64".into());
        let contract = ArtifactExecutionContract::from_spec(
            &artifact,
            Some(temp_path("gaia-node-provider-src").display().to_string()),
            false,
            ArtifactExecutionContract::default_command_policy(),
            gaia_spec::OutputRetentionPolicySpec::default(),
        );

        let error = NodeProvider
            .execute_artifact(&artifact, &contract, None, None)
            .expect_err("targeted node artifact should fail");

        assert_eq!(error.kind, ArtifactProviderErrorKind::PolicyBlocked);
        assert!(
            error
                .message
                .contains("target-aware builds are not supported yet")
        );
    }
}
