use gaia_artifact_providers::{
    ArtifactBackendState, ArtifactBatchItem, ArtifactExecutionContract, ArtifactProvider,
    ArtifactProviderError, ArtifactProviderErrorKind, ArtifactProviderValidationIssue,
    ProcessCancelCheck, ProcessLogLine, ProcessLogSink, ProcessLogStream, artifact_output_path,
    copy_artifact_file_to_output, render_artifact_backend_state, run_command_with_retries,
    write_artifact_marker,
};
use gaia_spec::{ArtifactDefinition, ArtifactSpec, BuildModeSpec, ResolvedBuildSpec};
use std::path::{Path, PathBuf};
use std::process::Command;

mod build_env;
mod cargo_errors;
mod cargo_git;
mod cargo_target;
mod crates;
mod feature_flags;
mod link_record;
mod link_trim;
mod std_library;
mod surface;
mod toolchain;
mod tools;

use build_env::{cargo_build_env, offline_cargo_env};
use cargo_errors::cargo_failing_packages;
use cargo_target::*;
use feature_flags::CargoFeatureFlags;

use tools::{host_toolchain_identity, sccache_env, strip_collected_output, toolchain_version};

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
        surface::validate(artifact, &mut issues);
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

        let flags = CargoFeatureFlags::of(artifact);
        let records_links = !flags.recorded.is_empty();
        let mut built_in = None;
        let (build_mode, messages) = if is_std_library(artifact) && contract.allow_nested_build {
            let target = cargo_target_for(source_dir, contract, records_links);
            std_library::collect(
                source_dir,
                contract,
                &target.dir,
                log_sink,
                cancel_check.clone(),
            )?;
            ("toolchain", Vec::new())
        } else if contract.allow_nested_build {
            let target = cargo_target_for(source_dir, contract, records_links);
            run_cargo_build(
                source_dir,
                &cargo_packages(artifact, &package),
                &flags,
                contract,
                &target,
                log_sink,
                cancel_check.clone(),
            )?;
            collect_cargo_output(source_dir, &target.dir, &package, contract, &target_name)?;
            built_in = Some(target.dir);
            ("cargo", Vec::new())
        } else if output_path.is_file() {
            ("existing-output", Vec::new())
        } else {
            // A placeholder file would be shipped as the binary. Without a
            // nested build there is nothing to collect, so say so.
            return Err(ArtifactProviderError::new(
                ArtifactProviderErrorKind::PolicyBlocked,
                format!(
                    "rust artifact '{}' has no output at '{}' and nested cargo builds are off; set [providers.rust] allow_nested_build = true to build it, or provide the output file",
                    artifact.id.as_str(),
                    contract.output.path
                ),
            ));
        };
        self.finish_artifact(
            artifact,
            contract,
            (&package, &target_name),
            build_mode,
            messages,
            built_in.as_deref().map(|dir| (dir, cancel_check)),
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
            "{group}features={:?}|source={:?}|target={:?}|toolchain={:?}|profile={:?}|backend={:?}|timeout={}|retries={}/{}/{:?}|jobs={:?}|retention={:?}",
            CargoFeatureFlags::of(artifact),
            contract.source_dir,
            contract.artifact_target,
            contract.toolchain,
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
        // Flags come from a crate-building member: the standard library's
        // own carry no features or link mode.
        let flags_from = items
            .iter()
            .find(|item| !is_std_library(item.artifact))
            .unwrap_or(leader);
        let flags = CargoFeatureFlags::of(flags_from.artifact);
        let target = cargo_target_for(source_dir, leader.contract, !flags.recorded.is_empty());
        let built = if packages.is_empty() {
            Ok(())
        } else {
            run_cargo_build(
                source_dir,
                &packages,
                &flags,
                leader.contract,
                &target,
                leader.log_sink.clone(),
                cancel_check.clone(),
            )
        };
        match built {
            Ok(()) => items
                .iter()
                .zip(resolved)
                .map(|(item, (package, target_name))| {
                    if is_std_library(item.artifact) {
                        std_library::collect(
                            source_dir,
                            item.contract,
                            &target.dir,
                            item.log_sink.clone(),
                            cancel_check.clone(),
                        )?;
                        return self.finish_artifact(
                            item.artifact,
                            item.contract,
                            (&package, &target_name),
                            "toolchain",
                            Vec::new(),
                            None,
                        );
                    }
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
                        (&package, &target_name),
                        "cargo",
                        vec![note.clone()],
                        Some((&target.dir, cancel_check.clone())),
                    )
                })
                .collect(),
            Err(error) if error.kind == ArtifactProviderErrorKind::Cancelled => {
                items.iter().map(|_| Err(error.clone())).collect()
            }
            Err(error) => {
                // Cargo names the packages that failed to compile. Those
                // artifacts fail with cargo's error; the others are built on
                // their own, so each failure is attributed to its artifact.
                let failing = cargo_failing_packages(&error.message);
                let failed = items
                    .iter()
                    .zip(&resolved)
                    .map(|(item, (package, _))| {
                        cargo_packages(item.artifact, package)
                            .iter()
                            .any(|member| failing.contains(member))
                    })
                    .collect::<Vec<_>>();
                if !failed.iter().any(|failed| *failed) {
                    if let Some(sink) = &leader.log_sink {
                        sink(ProcessLogLine {
                            stream: ProcessLogStream::Stderr,
                            line: format!(
                                "batched cargo build failed ({}); building each artifact on its own",
                                error.message.lines().next().unwrap_or_default()
                            ),
                        });
                    }
                    return build_individually(cancel_check);
                }
                if let Some(sink) = &leader.log_sink {
                    sink(ProcessLogLine {
                        stream: ProcessLogStream::Stderr,
                        line: format!(
                            "batched cargo build failed ({}); the other artifacts are built on their own",
                            error.message.lines().next().unwrap_or_default()
                        ),
                    });
                }
                items
                    .iter()
                    .zip(failed)
                    .map(|(item, failed)| {
                        if failed {
                            Err(error.clone())
                        } else {
                            self.execute_artifact(
                                item.artifact,
                                item.contract,
                                item.log_sink.clone(),
                                cancel_check.clone(),
                            )
                        }
                    })
                    .collect()
            }
        }
    }
}

impl RustProvider {
    fn finish_artifact(
        &self,
        artifact: &ArtifactSpec,
        contract: &ArtifactExecutionContract,
        (package, target_name): (&str, &str),
        build_mode: &str,
        mut messages: Vec<String>,
        built_in: Option<(&Path, Option<ProcessCancelCheck>)>,
    ) -> Result<Vec<String>, ArtifactProviderError> {
        messages.extend(surface::check(artifact, contract)?);
        // The surface is checked as cargo linked it; the trim keeps a part.
        if let Some((target_dir, cancel_check)) = built_in {
            let profile_dir = link_trim::profile_dir(target_dir, contract);
            messages.extend(link_trim::trim(
                artifact,
                contract,
                target_dir,
                &profile_dir,
                cancel_check,
            )?);
        }
        crates::record_after_build(artifact, contract, package, build_mode)?;
        write_artifact_marker(
            contract,
            self.id(),
            artifact.id.as_str(),
            &[
                ("package", package),
                ("target", target_name),
                ("mode", build_mode),
            ],
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

/// Whether `artifact` is the toolchain's standard library (`package = "std"`).
fn is_std_library(artifact: &ArtifactSpec) -> bool {
    matches!(&artifact.definition, ArtifactDefinition::Rust(rust) if rust.is_std_library())
}

/// Packages one cargo invocation for `artifact` selects: every package of
/// its build group, so each member resolves features identically.
fn cargo_packages(artifact: &ArtifactSpec, package: &str) -> Vec<String> {
    match &artifact.definition {
        ArtifactDefinition::Rust(rust) => rust.cargo_packages(),
        _ => vec![package.to_string()],
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
    let git_env =
        cargo_git::cargo_git_env(&flags.git_sources, contract, source_dir, log_sink.as_ref())?;
    let sccache = sccache_env(contract, log_sink.as_ref());
    let build_env = cargo_build_env(&flags.build_env)?;
    let toolchain_env = toolchain::with_build_env_and_cpu_flags(
        &build_env,
        toolchain::cargo_env(contract, Path::new(source_dir))?,
        flags.cpu.as_ref(),
        contract.artifact_target.as_deref(),
        |name| std::env::var(name).ok(),
    );
    let toolchain_env = toolchain::with_link_flags(
        toolchain_env,
        flags.link,
        contract.artifact_target.as_deref(),
        |name| {
            build_env
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .or_else(|| std::env::var(name).ok())
        },
    );
    let recording = if flags.recorded.is_empty() {
        Vec::new()
    } else {
        // The variables cargo sees, the later winning: build_env, then the
        // toolchain's.
        let cargo_env = toolchain_env
            .iter()
            .chain(&build_env)
            .cloned()
            .collect::<Vec<_>>();
        link_record::recording_env(
            &target.dir,
            contract,
            source_dir,
            &cargo_env,
            &flags.recorded,
        )?
    };
    let mut command = cargo_build_command(source_dir, packages, flags, contract, &target.dir);
    if toolchain::sets_target_rustflags(&toolchain_env) {
        // Folded into the target's flags: cargo would use it instead of them.
        command.env_remove("RUSTFLAGS");
    }
    command.envs(build_env);
    command.envs(git_env.iter().map(|(key, value)| (key, value)));
    command.envs(sccache.iter().map(|(key, value)| (key, value)));
    command.envs(toolchain_env.iter().map(|(key, value)| (key, value)));
    command.envs(recording);
    command.envs(offline_cargo_env(contract.offline));
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
        if contract.rust_strip {
            strip_collected_output(&output_path, contract)?;
        }
    }
    Ok(())
}

pub(crate) fn cargo_profile_dir(contract: &ArtifactExecutionContract) -> &str {
    match contract.build_mode.as_ref() {
        Some(BuildModeSpec::Release) => "release",
        Some(BuildModeSpec::Debug) | None => "debug",
        Some(BuildModeSpec::Custom(other)) if other.eq_ignore_ascii_case("dev") => "debug",
        Some(BuildModeSpec::Custom(other)) if other.eq_ignore_ascii_case("debug") => "debug",
        Some(BuildModeSpec::Custom(other)) if other.eq_ignore_ascii_case("release") => "release",
        Some(BuildModeSpec::Custom(other)) => other.as_str(),
    }
}

pub(crate) fn cargo_output_dir(target_dir: &Path, artifact_target: Option<&str>) -> PathBuf {
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
            toolchain_version(contract, "rustc", &["--version"]),
        ),
    ];
    extra_fields.extend(flags.state_fields());
    if contract.rust_strip {
        extra_fields.push(("strip".to_string(), "true".to_string()));
    }
    render_artifact_backend_state(ArtifactBackendState {
        contract,
        provider_id,
        artifact_id,
        resolved_identifier_kind: "package-target",
        resolved_identifier: &format!("{package}:{target_name}"),
        output_class: "binary",
        build_tool: "cargo",
        build_tool_version: &toolchain_version(contract, "cargo", &["--version"]),
        extra_fields: &extra_fields,
    })
}

#[cfg(test)]
mod tests;
