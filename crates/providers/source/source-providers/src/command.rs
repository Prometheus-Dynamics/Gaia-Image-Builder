use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceExecutionContext {
    pub(crate) workspace_root: PathBuf,
    pub(crate) docker: Option<SourceDockerExecution>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceDockerExecution {
    pub(crate) image: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourceCommandPolicy {
    pub(crate) attempts: u32,
    pub(crate) retry_backoff_ms: u64,
    pub(crate) retry_backoff_strategy: RetryBackoffStrategySpec,
    pub(crate) timeout_seconds: u64,
    pub(crate) output_retention: ProcessOutputRetention,
}

pub(crate) fn command_output_with_timeout(
    command: &mut Command,
    execution: &SourceExecutionContext,
    timeout: Duration,
    label: &str,
    retention: ProcessOutputRetention,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Output, SourceProviderError> {
    let label = label.to_string();
    let wrapped_sink = log_sink.map(|sink| label_process_log_sink(label.clone(), sink));
    let mut exec_command = command_for_execution(command, execution)?;
    run_command_with_timeout_and_retention(
        &mut exec_command,
        timeout,
        &label,
        retention,
        wrapped_sink,
        cancel_check,
    )
    .map(|result| result.output)
    .map_err(|error| {
        let kind = match error.kind {
            ProcessRunErrorKind::ToolStart => SourceProviderErrorKind::ToolStart,
            ProcessRunErrorKind::Timeout => SourceProviderErrorKind::Timeout,
            ProcessRunErrorKind::Cancelled => SourceProviderErrorKind::Cancelled,
            ProcessRunErrorKind::RuntimeState => SourceProviderErrorKind::RuntimeState,
        };
        SourceProviderError::new(kind, error.message)
    })
}

pub(crate) fn execution_context(spec: &ResolvedBuildSpec) -> SourceExecutionContext {
    let workspace_root = PathBuf::from(&spec.workspace.root_dir);
    SourceExecutionContext {
        workspace_root: fs::canonicalize(&workspace_root).unwrap_or(workspace_root),
        docker: spec
            .policy
            .execution
            .docker
            .as_ref()
            .map(|docker| SourceDockerExecution {
                image: docker.image.clone(),
            }),
    }
}

pub(crate) fn process_output_retention(spec: &ResolvedBuildSpec) -> ProcessOutputRetention {
    let retention = spec.policy.execution.output_retention;
    ProcessOutputRetention {
        stdout_bytes: retention.stdout_bytes,
        stderr_bytes: retention.stderr_bytes,
        stdout_lines: retention.stdout_lines,
        stderr_lines: retention.stderr_lines,
    }
}

pub(crate) fn command_for_execution(
    command: &Command,
    execution: &SourceExecutionContext,
) -> Result<Command, SourceProviderError> {
    let Some(docker) = &execution.docker else {
        return Ok(gaia_process::clone_command(command));
    };
    if docker.image.trim().is_empty() {
        return Err(SourceProviderError::new(
            SourceProviderErrorKind::PolicyBlocked,
            "docker execution requires a non-empty image",
        ));
    }
    let spec = DockerRunSpec::discovered_mounts(
        docker.image.clone(),
        execution.workspace_root.clone(),
        command,
    );
    docker_run_command(command, &spec).map_err(|error| {
        SourceProviderError::new(SourceProviderErrorKind::PolicyBlocked, error.to_string())
    })
}

pub(crate) fn run_command_with_policy(
    mut command: Command,
    execution: &SourceExecutionContext,
    description: &str,
    policy: SourceCommandPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(), SourceProviderError> {
    let attempts = policy.attempts.max(1);
    let timeout = Duration::from_secs(policy.timeout_seconds.max(1));
    let mut last_error = String::new();
    for attempt in 1..=attempts {
        tracing::debug!(
            command_label = description,
            provider_domain = "source",
            backend = execution_backend(execution),
            docker_image = execution
                .docker
                .as_ref()
                .map(|docker| docker.image.as_str()),
            attempt,
            attempts,
            timeout_seconds = timeout.as_secs(),
            "running source provider command"
        );
        let output = command_output_with_timeout(
            &mut command,
            execution,
            timeout,
            description,
            policy.output_retention,
            log_sink.clone(),
            cancel_check.clone(),
        )?;

        if output.status.success() {
            tracing::debug!(
                command_label = description,
                provider_domain = "source",
                backend = execution_backend(execution),
                attempt,
                attempts,
                "source provider command succeeded"
            );
            return Ok(());
        }
        last_error = format!(
            "{description} failed on attempt {attempt}/{attempts}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        if attempt < attempts {
            let retry_backoff = retry_backoff_duration(
                policy.retry_backoff_strategy,
                policy.retry_backoff_ms,
                attempt,
            );
            tracing::warn!(
                command_label = description,
                provider_domain = "source",
                backend = execution_backend(execution),
                attempt,
                attempts,
                backoff_ms = retry_backoff.as_millis(),
                "source provider command failed; retrying"
            );
            if !sleep_with_cancel(retry_backoff, cancel_check.as_ref()) {
                tracing::warn!(
                    command_label = description,
                    provider_domain = "source",
                    backend = execution_backend(execution),
                    attempt,
                    attempts,
                    "source provider retry backoff cancelled"
                );
                return Err(SourceProviderError::new(
                    SourceProviderErrorKind::Cancelled,
                    format!("{description} cancelled during retry backoff"),
                ));
            }
        }
    }
    tracing::warn!(
        command_label = description,
        provider_domain = "source",
        backend = execution_backend(execution),
        attempts,
        "source provider command exhausted retries"
    );
    Err(SourceProviderError::backend_command(last_error))
}

fn execution_backend(execution: &SourceExecutionContext) -> &'static str {
    if execution.docker.is_some() {
        "docker"
    } else {
        "host"
    }
}

pub(crate) fn process_retry_backoff_strategy(
    strategy: RetryBackoffStrategySpec,
) -> ProcessRetryBackoffStrategy {
    match strategy {
        RetryBackoffStrategySpec::Fixed => ProcessRetryBackoffStrategy::Fixed,
        RetryBackoffStrategySpec::Exponential => ProcessRetryBackoffStrategy::Exponential,
    }
}

pub(crate) fn retry_backoff_duration(
    strategy: RetryBackoffStrategySpec,
    base_backoff_ms: u64,
    attempt: u32,
) -> Duration {
    process_retry_backoff_duration(
        process_retry_backoff_strategy(strategy),
        base_backoff_ms,
        attempt,
    )
}
