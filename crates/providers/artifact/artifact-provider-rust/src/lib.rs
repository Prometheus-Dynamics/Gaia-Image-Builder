use gaia_artifact_providers::{
    ArtifactBackendState, ArtifactBatchItem, ArtifactExecutionBackend, ArtifactExecutionContract,
    ArtifactPlan, ArtifactProvider, ArtifactProviderError, ArtifactProviderErrorKind,
    ArtifactProviderOperation, ArtifactProviderValidationIssue, ProcessCancelCheck, ProcessLogLine,
    ProcessLogSink, ProcessLogStream, artifact_output_path, command_version_line,
    copy_artifact_file_to_output, materialize_artifact_marker_and_state,
    materialize_artifact_output, render_artifact_backend_state, run_command_with_retries,
};
use gaia_process::register_docker_mount;
use gaia_spec::{ArtifactDefinition, ArtifactSpec, BuildModeSpec, ResolvedBuildSpec};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
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
            let target = cargo_target_for(source_dir, contract);
            run_cargo_build(
                source_dir,
                &cargo_packages(artifact, &package),
                &CargoFeatureFlags::of(artifact),
                contract,
                &target,
                log_sink,
                cancel_check,
            )?;
            collect_cargo_output(source_dir, &target.dir, &package, contract, &target_name)?;
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
    /// invocation (one container start, one dependency resolution). Members
    /// of a build group only batch with each other, so the batched invocation
    /// stays exactly the group's own.
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
        let group = match &artifact.definition {
            ArtifactDefinition::Rust(rust) => rust
                .build_group
                .as_deref()
                .map(|group| format!("group={group}|"))
                .unwrap_or_default(),
            _ => String::new(),
        };
        Some(format!(
            "{group}features={:?}|source={:?}|target={:?}|profile={:?}|backend={:?}|timeout={}|retries={}/{}/{:?}|jobs={:?}|retention={:?}",
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
        for (item, (package, _)) in items.iter().zip(&resolved) {
            for package in cargo_packages(item.artifact, package) {
                if !packages.contains(&package) {
                    packages.push(package);
                }
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
        let target = cargo_target_for(source_dir, leader.contract);
        match run_cargo_build(
            source_dir,
            &packages,
            &CargoFeatureFlags::of(leader.artifact),
            leader.contract,
            &target,
            leader.log_sink.clone(),
            cancel_check.clone(),
        ) {
            Ok(()) => items
                .iter()
                .zip(resolved)
                .map(|(item, (package, target_name))| {
                    collect_cargo_output(
                        leader.contract.source_dir.as_deref().unwrap_or("."),
                        &target.dir,
                        &package,
                        item.contract,
                        &target_name,
                    )?;
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

/// Packages one cargo invocation for `artifact` selects: every package of
/// its build group, so each member resolves features identically.
fn cargo_packages(artifact: &ArtifactSpec, package: &str) -> Vec<String> {
    match &artifact.definition {
        ArtifactDefinition::Rust(rust) => rust.cargo_packages(),
        _ => vec![package.to_string()],
    }
}

/// A source's own cargo target dir, `<source>/.gaia/cargo-target`.
fn cargo_target_dir(source_dir: &str) -> PathBuf {
    PathBuf::from(source_dir).join(".gaia").join("cargo-target")
}

/// Directory inside a shared target dir that records which directory each
/// local package was first built from.
const SHARED_OWNERS_DIR: &str = "gaia-owners";

/// Where one cargo invocation writes.
struct CargoTarget {
    dir: PathBuf,
    /// Whether `dir` is shared with other sources: it is created up front and
    /// registered as a docker mount.
    shared: bool,
    /// Logged before the build: the shared dir in use, or why the source
    /// keeps its own dir although a shared one was asked for.
    note: Option<String>,
}

fn cargo_target_for(source_dir: &str, contract: &ArtifactExecutionContract) -> CargoTarget {
    cargo_target_for_in(
        gaia_spec::user_cache_root().as_deref(),
        source_dir,
        contract,
    )
}

/// Chooses the target dir of a build of `source_dir`. With
/// `shared_target_dir` it is `<cache_root>/cargo-target/<key>`, shared by
/// every source built with the same toolchain, target, profile and backend.
/// Cargo identifies a package by name and version, so a source whose local
/// packages an earlier source in that dir built from another directory keeps
/// its own dir instead of reusing those outputs.
fn cargo_target_for_in(
    cache_root: Option<&Path>,
    source_dir: &str,
    contract: &ArtifactExecutionContract,
) -> CargoTarget {
    let own = cargo_target_dir(source_dir);
    if !contract.rust_shared_target_dir {
        return CargoTarget {
            dir: own,
            shared: false,
            note: None,
        };
    }
    let Some(root) = cache_root else {
        return CargoTarget {
            dir: own,
            shared: false,
            note: Some(
                "shared cargo target dir needs a user cache directory (GAIA_CACHE_DIR, XDG_CACHE_HOME or HOME); using the source's own"
                    .into(),
            ),
        };
    };
    let dir = root.join("cargo-target").join(shared_target_key(contract));
    match claim_local_packages(source_dir, &dir) {
        Ok(None) => CargoTarget {
            note: Some(format!(
                "cargo target dir shared with other sources: {}",
                dir.display()
            )),
            dir,
            shared: true,
        },
        Ok(Some(conflict)) => CargoTarget {
            dir: own,
            shared: false,
            note: Some(format!(
                "{conflict}; using the source's own cargo target dir"
            )),
        },
        Err(reason) => CargoTarget {
            dir: own,
            shared: false,
            note: Some(format!("shared cargo target dir not used: {reason}")),
        },
    }
}

/// Name of a shared target dir: a hash of the toolchain (the host rustc and
/// cargo, or the docker image), target triple, profile and execution backend.
/// Cargo already keys its outputs by rustc and target, so the key only keeps
/// unrelated builds in separate dirs.
fn shared_target_key(contract: &ArtifactExecutionContract) -> String {
    let (backend, toolchain) = match &contract.execution_backend {
        ArtifactExecutionBackend::Host => ("host", host_toolchain_identity()),
        ArtifactExecutionBackend::Docker(docker) => ("docker", format!("image={}", docker.image)),
    };
    let identity = format!(
        "backend={backend}\ntoolchain={toolchain}\ntarget={}\nprofile={}\n",
        contract.artifact_target.as_deref().unwrap_or("host"),
        cargo_profile_dir(contract),
    );
    let digest = Sha256::digest(identity.as_bytes());
    digest
        .iter()
        .take(12)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn host_toolchain_identity() -> String {
    let rustc = Command::new("rustc")
        .arg("-vV")
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_else(|| "rustc unavailable".into());
    format!("{rustc}{}", command_version_line("cargo", &["--version"]))
}

/// Claims the local packages of `source_dir` (workspace and path packages;
/// registry and git packages are immutable and shared freely) for
/// `shared_dir`. Returns the first package claimed by another directory, or
/// `Err` when the packages cannot be listed or claimed.
fn claim_local_packages(source_dir: &str, shared_dir: &Path) -> Result<Option<String>, String> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .current_dir(source_dir)
        .output()
        .map_err(|error| format!("cargo metadata did not start: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "cargo metadata failed: {}",
            stderr.lines().next().unwrap_or("no output")
        ));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("cargo metadata output is not JSON: {error}"))?;
    let owners = shared_dir.join(SHARED_OWNERS_DIR);
    fs::create_dir_all(&owners)
        .map_err(|error| format!("failed to create '{}': {error}", owners.display()))?;
    for package in metadata["packages"].as_array().into_iter().flatten() {
        if !package["source"].is_null() {
            continue;
        }
        let (Some(name), Some(version), Some(manifest)) = (
            package["name"].as_str(),
            package["version"].as_str(),
            package["manifest_path"].as_str(),
        ) else {
            continue;
        };
        let Some(dir) = Path::new(manifest).parent() else {
            continue;
        };
        let dir = fs::canonicalize(dir)
            .unwrap_or_else(|_| dir.to_path_buf())
            .display()
            .to_string();
        let claim = owners.join(format!("{name}-{version}"));
        match OpenOptions::new().write(true).create_new(true).open(&claim) {
            Ok(mut file) => file
                .write_all(dir.as_bytes())
                .map_err(|error| format!("failed to write '{}': {error}", claim.display()))?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let owner = fs::read_to_string(&claim)
                    .map_err(|error| format!("failed to read '{}': {error}", claim.display()))?;
                if owner != dir {
                    return Ok(Some(format!(
                        "package '{name}' {version} is built from '{owner}' in the shared target dir"
                    )));
                }
            }
            Err(error) => {
                return Err(format!("failed to claim '{}': {error}", claim.display()));
            }
        }
    }
    Ok(None)
}

/// Creates a shared target dir and registers it as a docker mount, so
/// containers see it at its own path. Per-source dirs need nothing.
fn prepare_cargo_target(target: &CargoTarget) -> Result<(), ArtifactProviderError> {
    if !target.shared {
        return Ok(());
    }
    fs::create_dir_all(&target.dir).map_err(|error| {
        ArtifactProviderError::new(
            ArtifactProviderErrorKind::RuntimeState,
            format!(
                "failed to create shared cargo target dir '{}': {error}",
                target.dir.display()
            ),
        )
    })?;
    register_docker_mount(&target.dir);
    Ok(())
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
    target_dir: &Path,
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
    command.arg("--target-dir").arg(target_dir);
    command.current_dir(source_dir);
    command
}

fn run_cargo_build(
    source_dir: &str,
    packages: &[String],
    flags: &CargoFeatureFlags,
    contract: &ArtifactExecutionContract,
    target: &CargoTarget,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(), ArtifactProviderError> {
    prepare_cargo_target(target)?;
    if let (Some(note), Some(sink)) = (&target.note, &log_sink) {
        sink(ProcessLogLine {
            stream: ProcessLogStream::Stdout,
            line: note.clone(),
        });
    }
    let command = cargo_build_command(source_dir, packages, flags, contract, &target.dir);
    let label = match packages {
        [package] => format!("cargo build for package '{package}'"),
        _ => format!("cargo build for packages '{}'", packages.join("', '")),
    };
    run_command_with_retries(&command, contract, &label, log_sink, cancel_check)
}

fn collect_cargo_output(
    source_dir: &str,
    target_dir: &Path,
    package: &str,
    contract: &ArtifactExecutionContract,
    target_name: &str,
) -> Result<(), ArtifactProviderError> {
    let built_path = cargo_output_dir(target_dir, contract.artifact_target.as_deref())
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
mod tests;
