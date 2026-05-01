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
    match &contract.execution_backend {
        ArtifactExecutionBackend::Host => Ok(gaia_process::clone_command(command)),
        ArtifactExecutionBackend::Docker(docker) => docker_command(command, contract, docker),
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
    let spec = DockerRunSpec::workspace_mount(
        docker.image.clone(),
        PathBuf::from(workspace_root),
        command,
    )
    .with_extra_env("HOME", docker_home)
    .with_extra_env("XDG_CACHE_HOME", docker_cache);
    docker_run_command(command, &spec).map_err(|error| {
        ArtifactProviderError::new(ArtifactProviderErrorKind::PolicyBlocked, error.to_string())
    })
}
