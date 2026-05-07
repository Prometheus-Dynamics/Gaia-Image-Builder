use gaia_artifact_providers::{
    ArtifactBackendState, ArtifactExecutionContract, ArtifactPlan, ArtifactProvider,
    ArtifactProviderError, ArtifactProviderErrorKind, ArtifactProviderOperation,
    ArtifactProviderValidationIssue, ProcessCancelCheck, ProcessLogSink, artifact_output_path,
    command_version_line, copy_artifact_file_to_output, materialize_artifact_marker_and_state,
    materialize_artifact_output, render_artifact_backend_state, run_command_with_retries,
};
use gaia_spec::{ArtifactDefinition, ArtifactSpec, BuildModeSpec, ResolvedBuildSpec};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct RustProvider;

impl ArtifactProvider for RustProvider {
    fn id(&self) -> &'static str {
        "artifact.rust"
    }

    fn kind(&self) -> gaia_spec::ArtifactProviderKind {
        gaia_spec::ArtifactProviderKind::Rust
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
        if let ArtifactDefinition::Rust(rust) = &artifact.definition
            && let Some(target_name) = &rust.target_name
            && target_name.trim().is_empty()
        {
            issues.push(ArtifactProviderValidationIssue {
                code: "rust_target_name_empty",
                message: "rust target_name cannot be empty when set".into(),
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
        let (package, target_name) = match &artifact.definition {
            ArtifactDefinition::Rust(rust) => (
                rust.package.clone(),
                rust.target_name.clone().unwrap_or_else(|| {
                    Path::new(&contract.output.path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or(&rust.package)
                        .to_string()
                }),
            ),
            _ => (
                artifact.id.as_str().to_string(),
                artifact.id.as_str().to_string(),
            ),
        };

        let source_dir = contract.source_dir.as_deref().unwrap_or(".");
        let output_path = artifact_output_path(contract, source_dir);

        let (build_mode, mut messages) = if contract.allow_nested_build {
            (
                "cargo",
                build_with_cargo(
                    source_dir,
                    &package,
                    contract,
                    &target_name,
                    log_sink,
                    cancel_check,
                )?,
            )
        } else if output_path.is_file() {
            ("existing-output", Vec::new())
        } else {
            materialize_artifact_output(
                contract,
                &format!(
                    "provider={}\nartifact={}\npackage={package}\ntarget={target_name}\nmode=placeholder\n",
                    self.id(),
                    artifact.id.as_str()
                ),
            )?;
            (
                "placeholder",
                vec![format!(
                    "placeholder artifact output materialized for '{}'",
                    artifact.id.as_str()
                )],
            )
        };

        materialize_artifact_marker_and_state(
            contract,
            &format!(
                "provider={}\nartifact={}\npackage={package}\ntarget={target_name}\nmode={build_mode}\n",
                self.id(),
                artifact.id.as_str()
            ),
            &artifact_state_contents(
                self.id(),
                artifact.id.as_str(),
                contract,
                &package,
                &target_name,
                build_mode,
            ),
        )?;
        messages.push(format!(
            "rust artifact '{}' resolved package '{}' target '{}' -> {} ({build_mode})",
            artifact.id.as_str(),
            package,
            target_name,
            contract.output.path
        ));
        Ok(messages)
    }
}

fn build_with_cargo(
    source_dir: &str,
    package: &str,
    contract: &ArtifactExecutionContract,
    target_name: &str,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Vec<String>, ArtifactProviderError> {
    let target_dir = PathBuf::from(source_dir).join(".gaia").join("cargo-target");
    let cargo_target = contract.artifact_target.as_deref();

    let mut command = Command::new("cargo");
    command.arg("build").arg("-p").arg(package);
    if let Some(mode) = &contract.build_mode {
        match mode {
            BuildModeSpec::Release => {
                command.arg("--release");
            }
            BuildModeSpec::Debug => {}
            BuildModeSpec::Custom(profile) => {
                command.arg("--profile").arg(profile);
            }
        }
    }
    if let Some(target) = cargo_target {
        command.arg("--target").arg(target);
    }
    command.arg("--target-dir").arg(&target_dir);
    command.current_dir(source_dir);
    run_command_with_retries(
        &command,
        contract,
        &format!("cargo build for package '{package}'"),
        log_sink,
        cancel_check,
    )?;

    let profile_dir = cargo_profile_dir(contract);
    let built_path = cargo_output_dir(&target_dir, cargo_target)
        .join(profile_dir)
        .join(target_name);

    if !built_path.is_file() {
        return Err(ArtifactProviderError::new(
            ArtifactProviderErrorKind::OutputMissing,
            format!(
                "cargo build for package '{}' completed but expected artifact '{}' was not found",
                package,
                built_path.display()
            ),
        ));
    }

    let output_path = artifact_output_path(contract, source_dir);
    let built_canonical = built_path.canonicalize().ok();
    let output_canonical = output_path.canonicalize().ok();
    if built_canonical != output_canonical {
        copy_artifact_file_to_output(&built_path, &output_path, "built rust artifact")?;
    }
    Ok(Vec::new())
}

fn cargo_profile_dir(contract: &ArtifactExecutionContract) -> &str {
    match contract.build_mode.as_ref() {
        Some(BuildModeSpec::Release) => "release",
        Some(BuildModeSpec::Debug) | None => "debug",
        Some(BuildModeSpec::Custom(other)) if other.eq_ignore_ascii_case("dev") => "debug",
        Some(BuildModeSpec::Custom(other)) if other.eq_ignore_ascii_case("debug") => "debug",
        Some(BuildModeSpec::Custom(other)) if other.eq_ignore_ascii_case("release") => "release",
        Some(BuildModeSpec::Custom(other)) => other.as_str(),
    }
}

fn cargo_output_dir(target_dir: &Path, artifact_target: Option<&str>) -> PathBuf {
    match artifact_target {
        Some(target) => target_dir.join(target),
        None => target_dir.to_path_buf(),
    }
}

fn artifact_state_contents(
    provider_id: &str,
    artifact_id: &str,
    contract: &ArtifactExecutionContract,
    package: &str,
    target_name: &str,
    build_mode: &str,
) -> String {
    render_artifact_backend_state(ArtifactBackendState {
        contract,
        provider_id,
        artifact_id,
        resolved_identifier_kind: "package-target",
        resolved_identifier: &format!("{package}:{target_name}"),
        output_class: "binary",
        build_tool: "cargo",
        build_tool_version: &command_version_line("cargo", &["--version"]),
        extra_fields: &[
            ("package".to_string(), package.to_string()),
            ("target".to_string(), target_name.to_string()),
            (
                "artifact_target".to_string(),
                contract.artifact_target.clone().unwrap_or_default(),
            ),
            (
                "build_mode".to_string(),
                cargo_profile_dir(contract).to_string(),
            ),
            ("mode".to_string(), build_mode.to_string()),
            ("compiler_tool".to_string(), "rustc".to_string()),
            (
                "compiler_tool_version".to_string(),
                command_version_line("rustc", &["--version"]),
            ),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_spec::{ArtifactOutputSpec, ArtifactVariantSpec, RustArtifactSpec};
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
    fn rust_provider_fails_for_missing_source_dir_when_nested_build_enabled() {
        let output_path = temp_path("gaia-rust-provider-output");
        let missing_source_dir = temp_path("gaia-rust-provider-missing-source");
        let artifact = ArtifactSpec::new(
            "gaia-app",
            ArtifactDefinition::Rust(RustArtifactSpec {
                package: "gaia".into(),
                target_name: Some("gaia".into()),
                variant: ArtifactVariantSpec::File,
            }),
            None,
            ArtifactOutputSpec {
                path: output_path.display().to_string(),
            },
        );
        let contract = ArtifactExecutionContract::from_spec(
            &artifact,
            Some(missing_source_dir.display().to_string()),
            true,
            ArtifactExecutionContract::default_command_policy(),
            gaia_spec::OutputRetentionPolicySpec::default(),
        );

        let error = RustProvider
            .execute_artifact(&artifact, &contract, None, None)
            .expect_err("missing source dir should fail");

        assert_eq!(
            error.kind,
            gaia_artifact_providers::ArtifactProviderErrorKind::ToolStart
        );
        assert!(error.message.contains("failed to start cargo build"));
    }

    #[test]
    fn rust_artifact_state_persists_backend_native_fields() {
        let output_path = temp_path("gaia-rust-provider-state");
        fs::write(&output_path, "artifact").expect("output");
        let mut artifact = ArtifactSpec::new(
            "gaia-app",
            ArtifactDefinition::Rust(RustArtifactSpec {
                package: "gaia".into(),
                target_name: Some("gaia".into()),
                variant: ArtifactVariantSpec::File,
            }),
            None,
            ArtifactOutputSpec {
                path: output_path.display().to_string(),
            },
        );
        artifact.target = Some("aarch64-unknown-linux-gnu".into());
        let contract = ArtifactExecutionContract::from_spec(
            &artifact,
            None,
            false,
            ArtifactExecutionContract::default_command_policy(),
            gaia_spec::OutputRetentionPolicySpec::default(),
        );

        let state = artifact_state_contents(
            "artifact.rust",
            artifact.id.as_str(),
            &contract,
            "gaia",
            "gaia",
            "cargo",
        );

        assert!(state.contains("resolved_identifier_kind=package-target"));
        assert!(state.contains("resolved_identifier=gaia:gaia"));
        assert!(state.contains("produced_filename="));
        assert!(state.contains("output_class=binary"));
        assert!(state.contains("build_tool=cargo"));
        assert!(state.contains("compiler_tool=rustc"));
        assert!(state.contains("artifact_target=aarch64-unknown-linux-gnu"));
    }

    #[test]
    fn nested_rust_build_does_not_accept_stale_existing_output() {
        let output_path = temp_path("gaia-rust-provider-stale-output");
        fs::write(&output_path, "stale").expect("output");
        let missing_source_dir = temp_path("gaia-rust-provider-missing-source-with-output");
        let artifact = ArtifactSpec::new(
            "gaia-app",
            ArtifactDefinition::Rust(RustArtifactSpec {
                package: "gaia".into(),
                target_name: Some("gaia".into()),
                variant: ArtifactVariantSpec::File,
            }),
            None,
            ArtifactOutputSpec {
                path: output_path.display().to_string(),
            },
        );
        let contract = ArtifactExecutionContract::from_spec(
            &artifact,
            Some(missing_source_dir.display().to_string()),
            true,
            ArtifactExecutionContract::default_command_policy(),
            gaia_spec::OutputRetentionPolicySpec::default(),
        );

        let error = RustProvider
            .execute_artifact(&artifact, &contract, None, None)
            .expect_err("nested build should not reuse stale output");

        assert_eq!(
            error.kind,
            gaia_artifact_providers::ArtifactProviderErrorKind::ToolStart
        );
        assert!(error.message.contains("failed to start cargo build"));
    }
}
