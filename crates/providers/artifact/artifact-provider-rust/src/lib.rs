use gaia_artifact_providers::{
    ArtifactBackendState, ArtifactBatchItem, ArtifactExecutionContract, ArtifactPlan,
    ArtifactProvider, ArtifactProviderError, ArtifactProviderErrorKind, ArtifactProviderOperation,
    ArtifactProviderValidationIssue, ProcessCancelCheck, ProcessLogLine, ProcessLogSink,
    ProcessLogStream, artifact_output_path, command_version_line, copy_artifact_file_to_output,
    materialize_artifact_marker_and_state, materialize_artifact_output,
    render_artifact_backend_state, run_command_with_retries,
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
        if let ArtifactDefinition::Rust(rust) = &artifact.definition {
            if rust.all_features && (!rust.features.is_empty() || rust.no_default_features) {
                issues.push(ArtifactProviderValidationIssue {
                    code: "rust_features_conflict",
                    message:
                        "rust all_features cannot be combined with features or no_default_features"
                            .into(),
                });
            }
            if rust
                .features
                .iter()
                .any(|feature| feature.trim().is_empty())
            {
                issues.push(ArtifactProviderValidationIssue {
                    code: "rust_feature_empty",
                    message: "rust features cannot contain empty names".into(),
                });
            }
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
        let (package, target_name) = package_and_target(artifact, contract);
        let source_dir = contract.source_dir.as_deref().unwrap_or(".");
        let output_path = artifact_output_path(contract, source_dir);

        let (build_mode, messages) = if contract.allow_nested_build {
            run_cargo_build(
                source_dir,
                std::slice::from_ref(&package),
                &CargoFeatureFlags::of(artifact),
                contract,
                log_sink,
                cancel_check,
            )?;
            collect_cargo_output(source_dir, &package, contract, &target_name)?;
            ("cargo", Vec::new())
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
        self.finish_artifact(
            artifact,
            contract,
            &package,
            &target_name,
            build_mode,
            messages,
        )
    }

    /// Nested cargo builds from the same workspace, target triple, profile
    /// and execution backend can share one `cargo build -p a -p b ...`
    /// invocation (one container start, one dependency resolution).
    fn batch_key(
        &self,
        artifact: &ArtifactSpec,
        contract: &ArtifactExecutionContract,
    ) -> Option<String> {
        if !contract.allow_nested_build
            || !matches!(artifact.definition, ArtifactDefinition::Rust(_))
        {
            return None;
        }
        // cargo unifies features across every `-p` in one invocation, so only
        // artifacts with identical feature flags share a build.
        Some(format!(
            "features={:?}|source={:?}|target={:?}|profile={:?}|backend={:?}|timeout={}|retries={}/{}/{:?}|jobs={:?}|retention={:?}",
            CargoFeatureFlags::of(artifact),
            contract.source_dir,
            contract.artifact_target,
            contract.build_mode,
            contract.execution_backend,
            contract.timeout_seconds,
            contract.retry_attempts,
            contract.retry_backoff_ms,
            contract.retry_backoff_strategy,
            contract.job_budget,
            contract.output_retention,
        ))
    }

    /// Builds every package in one cargo invocation, then copies each output
    /// and writes the same marker and state files as a single build would.
    /// If the combined build fails, each artifact is rebuilt on its own so
    /// the failure is attributed to the right artifact (incremental
    /// compilation keeps that cheap).
    fn execute_artifact_batch(
        &self,
        items: &[ArtifactBatchItem<'_>],
        cancel_check: Option<ProcessCancelCheck>,
    ) -> Vec<Result<Vec<String>, ArtifactProviderError>> {
        let build_individually = |cancel_check: Option<ProcessCancelCheck>| {
            items
                .iter()
                .map(|item| {
                    self.execute_artifact(
                        item.artifact,
                        item.contract,
                        item.log_sink.clone(),
                        cancel_check.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let Some(leader) = items.first().filter(|_| items.len() > 1) else {
            return build_individually(cancel_check);
        };
        let resolved = items
            .iter()
            .map(|item| package_and_target(item.artifact, item.contract))
            .collect::<Vec<_>>();
        let mut packages = Vec::<String>::new();
        for (package, _) in &resolved {
            if !packages.contains(package) {
                packages.push(package.clone());
            }
        }
        let source_dir = leader.contract.source_dir.as_deref().unwrap_or(".");
        let note = format!(
            "cargo build batched for package(s) {} (cargo output is shown under '{}')",
            packages.join(", "),
            leader.artifact.id.as_str()
        );
        for item in &items[1..] {
            if let Some(sink) = &item.log_sink {
                sink(ProcessLogLine {
                    stream: ProcessLogStream::Stdout,
                    line: note.clone(),
                });
            }
        }
        match run_cargo_build(
            source_dir,
            &packages,
            &CargoFeatureFlags::of(leader.artifact),
            leader.contract,
            leader.log_sink.clone(),
            cancel_check.clone(),
        ) {
            Ok(()) => items
                .iter()
                .zip(resolved)
                .map(|(item, (package, target_name))| {
                    let item_source_dir = item.contract.source_dir.as_deref().unwrap_or(".");
                    collect_cargo_output(item_source_dir, &package, item.contract, &target_name)?;
                    self.finish_artifact(
                        item.artifact,
                        item.contract,
                        &package,
                        &target_name,
                        "cargo",
                        vec![note.clone()],
                    )
                })
                .collect(),
            Err(error) if error.kind == ArtifactProviderErrorKind::Cancelled => {
                items.iter().map(|_| Err(error.clone())).collect()
            }
            Err(error) => {
                if let Some(sink) = &leader.log_sink {
                    sink(ProcessLogLine {
                        stream: ProcessLogStream::Stderr,
                        line: format!(
                            "batched cargo build failed ({}); building each artifact on its own",
                            error.message.lines().next().unwrap_or_default()
                        ),
                    });
                }
                build_individually(cancel_check)
            }
        }
    }
}

impl RustProvider {
    fn finish_artifact(
        &self,
        artifact: &ArtifactSpec,
        contract: &ArtifactExecutionContract,
        package: &str,
        target_name: &str,
        build_mode: &str,
        mut messages: Vec<String>,
    ) -> Result<Vec<String>, ArtifactProviderError> {
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
                package,
                target_name,
                build_mode,
                &CargoFeatureFlags::of(artifact),
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

fn package_and_target(
    artifact: &ArtifactSpec,
    contract: &ArtifactExecutionContract,
) -> (String, String) {
    match &artifact.definition {
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
    }
}

fn cargo_target_dir(source_dir: &str) -> PathBuf {
    PathBuf::from(source_dir).join(".gaia").join("cargo-target")
}

/// Cargo feature selection of a rust artifact.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CargoFeatureFlags {
    features: Vec<String>,
    no_default_features: bool,
    all_features: bool,
}

impl CargoFeatureFlags {
    fn of(artifact: &ArtifactSpec) -> Self {
        match &artifact.definition {
            ArtifactDefinition::Rust(rust) => Self {
                features: rust.features.clone(),
                no_default_features: rust.no_default_features,
                all_features: rust.all_features,
            },
            _ => Self::default(),
        }
    }

    fn apply(&self, command: &mut Command) {
        if self.all_features {
            command.arg("--all-features");
        }
        if self.no_default_features {
            command.arg("--no-default-features");
        }
        if !self.features.is_empty() {
            command.arg("--features").arg(self.features.join(","));
        }
    }

    /// State fields, only for non-default selections so existing artifact
    /// state (and therefore reuse) is unchanged for artifacts without flags.
    fn state_fields(&self) -> Vec<(String, String)> {
        let mut fields = Vec::new();
        if !self.features.is_empty() {
            fields.push(("features".to_string(), self.features.join(",")));
        }
        if self.no_default_features {
            fields.push(("no_default_features".to_string(), "true".to_string()));
        }
        if self.all_features {
            fields.push(("all_features".to_string(), "true".to_string()));
        }
        fields
    }
}

fn cargo_build_command(
    source_dir: &str,
    packages: &[String],
    flags: &CargoFeatureFlags,
    contract: &ArtifactExecutionContract,
) -> Command {
    let mut command = Command::new("cargo");
    command.arg("build");
    for package in packages {
        command.arg("-p").arg(package);
    }
    flags.apply(&mut command);
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
    if let Some(target) = contract.artifact_target.as_deref() {
        command.arg("--target").arg(target);
    }
    command
        .arg("--target-dir")
        .arg(cargo_target_dir(source_dir));
    command.current_dir(source_dir);
    command
}

fn run_cargo_build(
    source_dir: &str,
    packages: &[String],
    flags: &CargoFeatureFlags,
    contract: &ArtifactExecutionContract,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(), ArtifactProviderError> {
    let command = cargo_build_command(source_dir, packages, flags, contract);
    let label = match packages {
        [package] => format!("cargo build for package '{package}'"),
        _ => format!("cargo build for packages '{}'", packages.join("', '")),
    };
    run_command_with_retries(&command, contract, &label, log_sink, cancel_check)
}

fn collect_cargo_output(
    source_dir: &str,
    package: &str,
    contract: &ArtifactExecutionContract,
    target_name: &str,
) -> Result<(), ArtifactProviderError> {
    let built_path = cargo_output_dir(
        &cargo_target_dir(source_dir),
        contract.artifact_target.as_deref(),
    )
    .join(cargo_profile_dir(contract))
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
    Ok(())
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
    flags: &CargoFeatureFlags,
) -> String {
    let mut extra_fields = vec![
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
    ];
    extra_fields.extend(flags.state_fields());
    render_artifact_backend_state(ArtifactBackendState {
        contract,
        provider_id,
        artifact_id,
        resolved_identifier_kind: "package-target",
        resolved_identifier: &format!("{package}:{target_name}"),
        output_class: "binary",
        build_tool: "cargo",
        build_tool_version: &command_version_line("cargo", &["--version"]),
        extra_fields: &extra_fields,
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
                features: Vec::new(),
                no_default_features: false,
                all_features: false,
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
                features: Vec::new(),
                no_default_features: false,
                all_features: false,
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
            &CargoFeatureFlags::default(),
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
                features: Vec::new(),
                no_default_features: false,
                all_features: false,
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

    fn rust_artifact(id: &str, package: &str, output: &Path) -> ArtifactSpec {
        ArtifactSpec::new(
            id,
            ArtifactDefinition::Rust(RustArtifactSpec {
                package: package.into(),
                target_name: Some(package.into()),
                variant: ArtifactVariantSpec::File,
                features: Vec::new(),
                no_default_features: false,
                all_features: false,
            }),
            None,
            ArtifactOutputSpec {
                path: output.display().to_string(),
            },
        )
    }

    fn nested_contract(artifact: &ArtifactSpec, source_dir: &Path) -> ArtifactExecutionContract {
        ArtifactExecutionContract::from_spec(
            artifact,
            Some(source_dir.display().to_string()),
            true,
            ArtifactExecutionContract::default_command_policy(),
            gaia_spec::OutputRetentionPolicySpec::default(),
        )
    }

    /// A two-member cargo workspace with dependency-free binaries, so the
    /// build is fast and offline. `broken` members fail to compile.
    fn tiny_workspace(prefix: &str, members: &[(&str, bool)]) -> PathBuf {
        let root = temp_path(prefix);
        let names = members
            .iter()
            .map(|(name, _)| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(", ");
        fs::create_dir_all(&root).expect("workspace root");
        fs::write(
            root.join("Cargo.toml"),
            format!("[workspace]\nmembers = [{names}]\nresolver = \"2\"\n"),
        )
        .expect("workspace manifest");
        for (name, broken) in members {
            let dir = root.join(name);
            fs::create_dir_all(dir.join("src")).expect("member src");
            fs::write(
                dir.join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
            )
            .expect("member manifest");
            let body = if *broken {
                "fn main() { let x: u32 = \"not a number\"; }\n".to_string()
            } else {
                format!("fn main() {{ println!(\"{name}\"); }}\n")
            };
            fs::write(dir.join("src/main.rs"), body).expect("member main");
        }
        root
    }

    fn capture_sink() -> (
        ProcessLogSink,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) {
        let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = lines.clone();
        let sink: ProcessLogSink = std::sync::Arc::new(move |line: ProcessLogLine| {
            captured.lock().expect("lines").push(line.line);
        });
        (sink, lines)
    }

    #[test]
    fn batch_key_groups_only_compatible_nested_builds() {
        let source = temp_path("gaia-rust-batch-key");
        let alpha = rust_artifact("alpha", "alpha", &source.join("out/alpha"));
        let beta = rust_artifact("beta", "beta", &source.join("out/beta"));
        let alpha_contract = nested_contract(&alpha, &source);
        let beta_contract = nested_contract(&beta, &source);

        let alpha_key = RustProvider.batch_key(&alpha, &alpha_contract);
        assert!(alpha_key.is_some());
        assert_eq!(alpha_key, RustProvider.batch_key(&beta, &beta_contract));

        let mut release = beta_contract.clone();
        release.build_mode = Some(BuildModeSpec::Release);
        assert_ne!(alpha_key, RustProvider.batch_key(&beta, &release));
        let mut cross = beta_contract.clone();
        cross.artifact_target = Some("aarch64-unknown-linux-gnu".into());
        assert_ne!(alpha_key, RustProvider.batch_key(&beta, &cross));
        let mut other_source = beta_contract.clone();
        other_source.source_dir = Some(temp_path("other").display().to_string());
        assert_ne!(alpha_key, RustProvider.batch_key(&beta, &other_source));
        let mut placeholder = beta_contract;
        placeholder.allow_nested_build = false;
        assert_eq!(RustProvider.batch_key(&beta, &placeholder), None);
    }

    #[test]
    fn cargo_build_command_selects_every_batched_package() {
        let source = temp_path("gaia-rust-batch-command");
        let alpha = rust_artifact("alpha", "alpha", &source.join("out/alpha"));
        let mut contract = nested_contract(&alpha, &source);
        contract.build_mode = Some(BuildModeSpec::Release);
        let command = cargo_build_command(
            &source.display().to_string(),
            &["alpha".to_string(), "beta".to_string()],
            &CargoFeatureFlags::default(),
            &contract,
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(&args[..5], ["build", "-p", "alpha", "-p", "beta"]);
        assert!(args.contains(&"--release".to_string()));
    }

    #[test]
    fn batched_cargo_build_matches_individual_outputs_and_state() {
        let workspace = tiny_workspace("gaia-rust-batch-ok", &[("alpha", false), ("beta", false)]);
        let alpha = rust_artifact("alpha", "alpha", &workspace.join("out/alpha"));
        let beta = rust_artifact("beta", "beta", &workspace.join("out/beta"));
        let alpha_contract = nested_contract(&alpha, &workspace);
        let beta_contract = nested_contract(&beta, &workspace);
        let (leader_sink, leader_lines) = capture_sink();
        let (member_sink, member_lines) = capture_sink();

        let results = RustProvider.execute_artifact_batch(
            &[
                ArtifactBatchItem {
                    artifact: &alpha,
                    contract: &alpha_contract,
                    log_sink: Some(leader_sink),
                },
                ArtifactBatchItem {
                    artifact: &beta,
                    contract: &beta_contract,
                    log_sink: Some(member_sink),
                },
            ],
            None,
        );

        assert_eq!(results.len(), 2);
        for (result, artifact, contract, package) in [
            (&results[0], &alpha, &alpha_contract, "alpha"),
            (&results[1], &beta, &beta_contract, "beta"),
        ] {
            let messages = result.as_ref().expect("batched build succeeds");
            assert!(messages.last().expect("summary").ends_with("(cargo)"));
            assert!(workspace.join("out").join(package).is_file());
            let state = fs::read_to_string(gaia_artifact_providers::artifact_state_path(contract))
                .expect("state file");
            assert_eq!(
                state,
                artifact_state_contents(
                    "artifact.rust",
                    artifact.id.as_str(),
                    contract,
                    package,
                    package,
                    "cargo",
                    &CargoFeatureFlags::default(),
                )
            );
        }
        let leader_lines = leader_lines.lock().expect("lines");
        assert!(
            leader_lines.iter().any(|line| line.contains("alpha"))
                && leader_lines.iter().any(|line| line.contains("beta")),
            "one cargo invocation compiles both packages: {leader_lines:?}"
        );
        assert!(
            member_lines
                .lock()
                .expect("lines")
                .iter()
                .any(|line| line.contains("cargo build batched"))
        );
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn failed_batch_falls_back_to_individual_builds() {
        let workspace = tiny_workspace("gaia-rust-batch-fail", &[("good", false), ("bad", true)]);
        let good = rust_artifact("good", "good", &workspace.join("out/good"));
        let bad = rust_artifact("bad", "bad", &workspace.join("out/bad"));
        let good_contract = nested_contract(&good, &workspace);
        let bad_contract = nested_contract(&bad, &workspace);

        let results = RustProvider.execute_artifact_batch(
            &[
                ArtifactBatchItem {
                    artifact: &good,
                    contract: &good_contract,
                    log_sink: None,
                },
                ArtifactBatchItem {
                    artifact: &bad,
                    contract: &bad_contract,
                    log_sink: None,
                },
            ],
            None,
        );

        assert!(results[0].is_ok(), "{:?}", results[0]);
        assert!(workspace.join("out/good").is_file());
        let error = results[1].as_ref().expect_err("broken package fails");
        assert_eq!(error.kind, ArtifactProviderErrorKind::BackendCommand);
        assert!(error.message.contains("'bad'"), "{}", error.message);
        let _ = fs::remove_dir_all(workspace);
    }

    fn with_features(
        mut artifact: ArtifactSpec,
        features: &[&str],
        no_default_features: bool,
        all_features: bool,
    ) -> ArtifactSpec {
        if let ArtifactDefinition::Rust(rust) = &mut artifact.definition {
            rust.features = features.iter().map(|feature| feature.to_string()).collect();
            rust.no_default_features = no_default_features;
            rust.all_features = all_features;
        }
        artifact
    }

    #[test]
    fn cargo_command_passes_feature_flags_through() {
        let source = temp_path("gaia-rust-features-command");
        let artifact = with_features(
            rust_artifact("node", "orion-node", &source.join("out/node")),
            &["metrics", "tls"],
            true,
            false,
        );
        let contract = nested_contract(&artifact, &source);
        let args = cargo_build_command(
            &source.display().to_string(),
            &["orion-node".to_string()],
            &CargoFeatureFlags::of(&artifact),
            &contract,
        )
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
        assert_eq!(
            &args[..6],
            [
                "build",
                "-p",
                "orion-node",
                "--no-default-features",
                "--features",
                "metrics,tls"
            ]
        );

        let all = with_features(artifact, &[], false, true);
        let args = cargo_build_command(
            &source.display().to_string(),
            &["orion-node".to_string()],
            &CargoFeatureFlags::of(&all),
            &contract,
        )
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
        assert!(args.contains(&"--all-features".to_string()));
        assert!(!args.contains(&"--features".to_string()));
    }

    #[test]
    fn validation_rejects_all_features_with_other_feature_flags() {
        let base = rust_artifact("node", "orion-node", Path::new("/tmp/out/node"));
        for artifact in [
            with_features(base.clone(), &["tls"], false, true),
            with_features(base.clone(), &[], true, true),
        ] {
            assert!(
                RustProvider
                    .validate_artifact(&artifact)
                    .iter()
                    .any(|issue| issue.code == "rust_features_conflict")
            );
        }
        assert!(
            RustProvider
                .validate_artifact(&with_features(base, &["tls"], true, false))
                .is_empty()
        );
    }

    #[test]
    fn batching_and_state_respect_feature_flags() {
        let source = temp_path("gaia-rust-features-batch");
        let plain = rust_artifact("a", "a", &source.join("out/a"));
        let featured = with_features(
            rust_artifact("b", "b", &source.join("out/b")),
            &["x"],
            false,
            false,
        );
        let plain_key = RustProvider.batch_key(&plain, &nested_contract(&plain, &source));
        let featured_key = RustProvider.batch_key(&featured, &nested_contract(&featured, &source));
        assert!(plain_key.is_some() && featured_key.is_some());
        assert_ne!(
            plain_key, featured_key,
            "mixed feature sets must not share a cargo build"
        );

        let contract = nested_contract(&plain, &source);
        let plain_state = artifact_state_contents(
            "artifact.rust",
            "a",
            &contract,
            "a",
            "a",
            "cargo",
            &CargoFeatureFlags::of(&plain),
        );
        assert!(!plain_state.contains("\nfeatures="), "{plain_state}");
        assert!(!plain_state.contains("default_features="), "{plain_state}");
        let featured_state = artifact_state_contents(
            "artifact.rust",
            "b",
            &contract,
            "b",
            "b",
            "cargo",
            &CargoFeatureFlags::of(&featured),
        );
        assert!(
            featured_state.contains("\nfeatures=x\n"),
            "{featured_state}"
        );
    }

    #[test]
    fn no_default_features_reaches_cargo() {
        // The default feature makes the crate fail to compile, so the build
        // only succeeds when --no-default-features is passed through.
        let root = temp_path("gaia-rust-no-default-features");
        fs::create_dir_all(root.join("src")).expect("src");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"gated\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\ndefault = [\"broken\"]\nbroken = []\n\n[workspace]\n",
        )
        .expect("manifest");
        fs::write(
            root.join("src/main.rs"),
            "#[cfg(feature = \"broken\")]\ncompile_error!(\"default feature enabled\");\nfn main() {}\n",
        )
        .expect("main");
        let artifact = with_features(
            rust_artifact("gated", "gated", &root.join("out/gated")),
            &[],
            true,
            false,
        );
        let contract = nested_contract(&artifact, &root);

        RustProvider
            .execute_artifact(&artifact, &contract, None, None)
            .expect("build without default features succeeds");
        assert!(root.join("out/gated").is_file());

        let with_defaults = with_features(artifact, &[], false, false);
        let error = RustProvider
            .execute_artifact(&with_defaults, &contract, None, None)
            .expect_err("default features fail to compile");
        assert_eq!(error.kind, ArtifactProviderErrorKind::BackendCommand);
        let _ = fs::remove_dir_all(root);
    }
}
