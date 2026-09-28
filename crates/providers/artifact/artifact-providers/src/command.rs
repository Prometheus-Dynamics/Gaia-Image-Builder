use crate::{
    ArtifactDockerExecution, ArtifactExecutionBackend, ArtifactExecutionContract,
    ArtifactProviderError, ArtifactProviderErrorKind, ProcessCancelCheck, ProcessLogSink,
};
use gaia_process::{
    DockerRunSpec, ProcessOutputRetention, ProcessRunErrorKind, docker_run_command,
    run_command_with_timeout, run_command_with_timeout_and_retention,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

pub fn command_output_with_timeout(
    command: &mut Command,
    timeout: Duration,
    label: &str,
) -> Result<Output, ArtifactProviderError> {
    command_output_with_timeout_and_sink(command, timeout, label, None, None)
        .map(|result| result.output)
}

pub fn command_output_with_timeout_and_sink(
    command: &mut Command,
    timeout: Duration,
    label: &str,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<gaia_process::ProcessRunResult, ArtifactProviderError> {
    run_command_with_timeout(command, timeout, label, log_sink, cancel_check).map_err(|error| {
        let kind = match error.kind {
            ProcessRunErrorKind::ToolStart => ArtifactProviderErrorKind::ToolStart,
            ProcessRunErrorKind::Timeout => ArtifactProviderErrorKind::Timeout,
            ProcessRunErrorKind::Cancelled => ArtifactProviderErrorKind::Cancelled,
            ProcessRunErrorKind::RuntimeState => ArtifactProviderErrorKind::RuntimeState,
        };
        ArtifactProviderError::new(kind, error.message)
    })
}

pub fn command_output_with_timeout_sink_and_retention(
    command: &mut Command,
    timeout: Duration,
    label: &str,
    retention: ProcessOutputRetention,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<gaia_process::ProcessRunResult, ArtifactProviderError> {
    run_command_with_timeout_and_retention(
        command,
        timeout,
        label,
        retention,
        log_sink,
        cancel_check,
    )
    .map_err(|error| {
        let kind = match error.kind {
            ProcessRunErrorKind::ToolStart => ArtifactProviderErrorKind::ToolStart,
            ProcessRunErrorKind::Timeout => ArtifactProviderErrorKind::Timeout,
            ProcessRunErrorKind::Cancelled => ArtifactProviderErrorKind::Cancelled,
            ProcessRunErrorKind::RuntimeState => ArtifactProviderErrorKind::RuntimeState,
        };
        ArtifactProviderError::new(kind, error.message)
    })
}

pub fn command_for_execution(
    command: &Command,
    contract: &ArtifactExecutionContract,
) -> Result<Command, ArtifactProviderError> {
    let mut budgeted;
    let command = match contract.job_budget {
        Some(jobs) => {
            budgeted = gaia_process::clone_command(command);
            apply_job_budget(&mut budgeted, jobs);
            &budgeted
        }
        None => command,
    };
    match &contract.execution_backend {
        ArtifactExecutionBackend::Host => Ok(gaia_process::clone_command(command)),
        ArtifactExecutionBackend::Docker(docker) => docker_command(command, contract, docker),
    }
}

/// Environment variables that cap build tool parallelism.
const JOB_BUDGET_ENV: [&str; 3] = [
    "CARGO_BUILD_JOBS",
    "MAKEFLAGS",
    "CMAKE_BUILD_PARALLEL_LEVEL",
];

/// Caps the tool parallelism of `command` at `jobs` through the usual
/// environment variables. A variable the command or the calling environment
/// already sets is left alone, so users keep control.
pub fn apply_job_budget(command: &mut Command, jobs: usize) {
    let jobs = jobs.max(1);
    for key in JOB_BUDGET_ENV {
        let set_on_command = command.get_envs().any(|(name, _)| name == key);
        if set_on_command || std::env::var_os(key).is_some() {
            continue;
        }
        let value = if key == "MAKEFLAGS" {
            format!("-j{jobs}")
        } else {
            jobs.to_string()
        };
        command.env(key, value);
    }
}

pub fn run_command_with_retries(
    command: &Command,
    contract: &ArtifactExecutionContract,
    label: &str,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(), ArtifactProviderError> {
    let attempts = contract.retry_attempts.max(1);
    let timeout = Duration::from_secs(contract.timeout_seconds.max(1));
    let mut last_error = String::new();
    for attempt in 1..=attempts {
        tracing::debug!(
            command_label = label,
            provider_domain = "artifact",
            provider = %contract.provider.as_str(),
            output = %contract.output.path.as_str(),
            attempt,
            attempts,
            timeout_seconds = timeout.as_secs(),
            backend = execution_backend(contract),
            docker_image = docker_image(contract),
            "running artifact provider command"
        );
        let mut exec_command = command_for_execution(command, contract)?;
        let output = command_output_with_timeout_sink_and_retention(
            &mut exec_command,
            timeout,
            label,
            process_output_retention(contract),
            log_sink.clone(),
            cancel_check.clone(),
        )?;
        if output.output.status.success() {
            tracing::debug!(
                command_label = label,
                provider_domain = "artifact",
                provider = %contract.provider.as_str(),
                output = %contract.output.path.as_str(),
                attempt,
                attempts,
                backend = execution_backend(contract),
                "artifact provider command succeeded"
            );
            return Ok(());
        }
        last_error = format!(
            "{label} failed on attempt {attempt}/{attempts}: {}",
            String::from_utf8_lossy(&output.output.stderr).trim()
        );
        if attempt < attempts {
            let retry_backoff = crate::retry_backoff_duration(
                contract.retry_backoff_strategy,
                contract.retry_backoff_ms,
                attempt,
            );
            tracing::warn!(
                command_label = label,
                provider_domain = "artifact",
                provider = %contract.provider.as_str(),
                output = %contract.output.path.as_str(),
                attempt,
                attempts,
                backend = execution_backend(contract),
                backoff_ms = retry_backoff.as_millis(),
                "artifact provider command failed; retrying"
            );
            if !crate::sleep_with_cancel(retry_backoff, cancel_check.as_ref()) {
                tracing::warn!(
                    command_label = label,
                    provider_domain = "artifact",
                    provider = %contract.provider.as_str(),
                    output = %contract.output.path.as_str(),
                    attempt,
                    attempts,
                    backend = execution_backend(contract),
                    "artifact provider retry backoff cancelled"
                );
                return Err(ArtifactProviderError::new(
                    ArtifactProviderErrorKind::Cancelled,
                    format!("{label} cancelled during retry backoff"),
                ));
            }
        }
    }
    tracing::warn!(
        command_label = label,
        provider_domain = "artifact",
        provider = %contract.provider.as_str(),
        output = %contract.output.path.as_str(),
        attempts,
        backend = execution_backend(contract),
        "artifact provider command exhausted retries"
    );
    Err(ArtifactProviderError::new(
        ArtifactProviderErrorKind::BackendCommand,
        last_error,
    ))
}

/// Builds the Dockerfile-backed execution image when its content-addressed
/// tag is missing, and records the image id on the contract. A no-op for
/// host execution and plain `image` references.
pub fn ensure_docker_execution_image(
    contract: &mut ArtifactExecutionContract,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Vec<String>, ArtifactProviderError> {
    ensure_docker_execution_image_with(
        std::ffi::OsStr::new("docker"),
        contract,
        log_sink,
        cancel_check,
    )
}

pub(crate) fn ensure_docker_execution_image_with(
    docker_program: &std::ffi::OsStr,
    contract: &mut ArtifactExecutionContract,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Vec<String>, ArtifactProviderError> {
    let timeout = Duration::from_secs(contract.timeout_seconds.max(1));
    let retention = process_output_retention(contract);
    let ArtifactExecutionBackend::Docker(docker) = &mut contract.execution_backend else {
        return Ok(Vec::new());
    };
    let Some(build) = &mut docker.build else {
        return Ok(Vec::new());
    };
    if let Some(error) = &build.hash_error {
        return Err(ArtifactProviderError::new(
            ArtifactProviderErrorKind::PolicyBlocked,
            error.clone(),
        ));
    }
    let tag = docker.image.clone();
    let image_id = |label: &str| -> Result<Option<String>, ArtifactProviderError> {
        let mut inspect = gaia_process::docker_image_id_command(docker_program, &tag);
        let output = command_output_with_timeout_sink_and_retention(
            &mut inspect,
            Duration::from_secs(60),
            label,
            retention,
            None,
            cancel_check.clone(),
        )?
        .output;
        let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok((output.status.success() && !id.is_empty()).then_some(id))
    };
    if let Some(id) = image_id("inspect docker execution image")? {
        build.image_id = Some(id);
        return Ok(vec![format!(
            "docker execution image '{tag}' is up to date"
        )]);
    }
    let mut command = gaia_process::docker_image_build_command(
        docker_program,
        Path::new(&build.dockerfile),
        Path::new(build.context.as_deref().unwrap_or(".")),
        &tag,
    );
    tracing::info!(image = %tag, dockerfile = %build.dockerfile, "building docker execution image");
    let output = command_output_with_timeout_sink_and_retention(
        &mut command,
        timeout,
        "build docker execution image",
        retention,
        log_sink,
        cancel_check.clone(),
    )?
    .output;
    if !output.status.success() {
        return Err(ArtifactProviderError::backend_command(format!(
            "docker build of '{}' for image '{tag}' failed: {}",
            build.dockerfile,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let id = image_id("inspect built docker execution image")?.ok_or_else(|| {
        ArtifactProviderError::backend_command(format!(
            "docker build reported success but image '{tag}' is missing"
        ))
    })?;
    build.image_id = Some(id);
    Ok(vec![format!(
        "built docker execution image '{tag}' from '{}'",
        build.dockerfile
    )])
}

fn execution_backend(contract: &ArtifactExecutionContract) -> &'static str {
    match contract.execution_backend {
        ArtifactExecutionBackend::Host => "host",
        ArtifactExecutionBackend::Docker(_) => "docker",
    }
}

fn docker_image(contract: &ArtifactExecutionContract) -> Option<&str> {
    match &contract.execution_backend {
        ArtifactExecutionBackend::Host => None,
        ArtifactExecutionBackend::Docker(docker) => Some(docker.image.as_str()),
    }
}

fn process_output_retention(contract: &ArtifactExecutionContract) -> ProcessOutputRetention {
    ProcessOutputRetention {
        stdout_bytes: contract.output_retention.stdout_bytes,
        stderr_bytes: contract.output_retention.stderr_bytes,
        stdout_lines: contract.output_retention.stdout_lines,
        stderr_lines: contract.output_retention.stderr_lines,
    }
}

fn docker_command(
    command: &Command,
    contract: &ArtifactExecutionContract,
    docker: &ArtifactDockerExecution,
) -> Result<Command, ArtifactProviderError> {
    if docker.image.trim().is_empty() {
        return Err(ArtifactProviderError::new(
            ArtifactProviderErrorKind::PolicyBlocked,
            "docker execution requires a non-empty image",
        ));
    }
    let workspace_root = contract.workspace_root.as_deref().ok_or_else(|| {
        ArtifactProviderError::new(
            ArtifactProviderErrorKind::RuntimeState,
            "docker execution requires a resolved workspace root",
        )
    })?;
    let docker_home = Path::new(workspace_root).join(".gaia/docker-home");
    let docker_cache = Path::new(workspace_root).join(".gaia/docker-cache");
    fs::create_dir_all(&docker_home).map_err(|error| {
        ArtifactProviderError::new(
            ArtifactProviderErrorKind::RuntimeState,
            format!(
                "failed to create docker home dir '{}': {error}",
                docker_home.display()
            ),
        )
    })?;
    fs::create_dir_all(&docker_cache).map_err(|error| {
        ArtifactProviderError::new(
            ArtifactProviderErrorKind::RuntimeState,
            format!(
                "failed to create docker cache dir '{}': {error}",
                docker_cache.display()
            ),
        )
    })?;
    let mut spec = DockerRunSpec::workspace_mount(
        docker.image.clone(),
        PathBuf::from(workspace_root),
        command,
    )
    .with_extra_env("HOME", &docker_home)
    .with_extra_env("XDG_CACHE_HOME", &docker_cache);
    // Containers run with --rm, so tool caches inside the image are lost after
    // every build. Keep them in the workspace cache unless the artifact sets
    // its own location.
    for (key, cache_dir) in [("CARGO_HOME", "cargo"), ("SCCACHE_DIR", "sccache")] {
        let already_set = command
            .get_envs()
            .any(|(name, value)| name == key && value.is_some());
        if !already_set {
            spec = spec.with_extra_env(key, docker_cache.join(cache_dir));
        }
    }
    docker_run_command(command, &spec).map_err(|error| {
        ArtifactProviderError::new(ArtifactProviderErrorKind::PolicyBlocked, error.to_string())
    })
}
