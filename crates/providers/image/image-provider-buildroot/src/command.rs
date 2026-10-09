use super::*;

/// Runs a command (with the policy's retries and timeout), returning its
/// step time; a failure carries it too.
pub(crate) fn run_command(
    command: Command,
    label: &str,
    execution: &ImageExecutionContext,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Vec<String>, ImageProviderError> {
    let started = std::time::Instant::now();
    let step_time = || gaia_process::step_time_message(label, started.elapsed());
    match run_command_attempts(command, label, execution, policy, log_sink, cancel_check) {
        Ok(()) => Ok(vec![step_time()]),
        Err(error) => Err(error.with_step_times([step_time()])),
    }
}

fn run_command_attempts(
    mut command: Command,
    label: &str,
    execution: &ImageExecutionContext,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(), ImageProviderError> {
    let attempts = policy.retry_attempts.max(1);
    let timeout = Duration::from_secs(policy.timeout_seconds.max(1));
    let mut last_error = String::new();
    for attempt in 1..=attempts {
        tracing::debug!(
            command_label = label,
            provider_domain = "image.buildroot",
            backend = execution_backend(execution),
            docker_image = execution.docker_image.as_deref(),
            attempt,
            attempts,
            timeout_seconds = timeout.as_secs(),
            "running image provider command"
        );
        let output = command_output_with_timeout(
            &mut command,
            execution,
            timeout,
            label,
            policy.output_retention,
            log_sink.clone(),
            cancel_check.clone(),
        )?;
        if output.status.success() {
            tracing::debug!(
                command_label = label,
                provider_domain = "image.buildroot",
                backend = execution_backend(execution),
                attempt,
                attempts,
                "image provider command succeeded"
            );
            return Ok(());
        }
        last_error = format!(
            "{label} failed on attempt {attempt}/{attempts}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        if attempt < attempts {
            let retry_backoff = retry_backoff_duration(
                policy.retry_backoff_strategy,
                policy.retry_backoff_ms,
                attempt,
            );
            tracing::warn!(
                command_label = label,
                provider_domain = "image.buildroot",
                backend = execution_backend(execution),
                attempt,
                attempts,
                backoff_ms = retry_backoff.as_millis(),
                "image provider command failed; retrying"
            );
            if !sleep_with_cancel(retry_backoff, cancel_check.as_ref()) {
                tracing::warn!(
                    command_label = label,
                    provider_domain = "image.buildroot",
                    backend = execution_backend(execution),
                    attempt,
                    attempts,
                    "image provider retry backoff cancelled"
                );
                return Err(ImageProviderError::new(
                    ImageProviderErrorKind::Cancelled,
                    format!("{label} cancelled during retry backoff"),
                ));
            }
        }
    }
    tracing::warn!(
        command_label = label,
        provider_domain = "image.buildroot",
        backend = execution_backend(execution),
        attempts,
        "image provider command exhausted retries"
    );
    Err(ImageProviderError::backend_command(last_error))
}

fn execution_backend(execution: &ImageExecutionContext) -> &'static str {
    if execution.docker_image.is_some() {
        "docker"
    } else {
        "host"
    }
}

pub(crate) fn retry_backoff_duration(
    strategy: RetryBackoffStrategySpec,
    base_backoff_ms: u64,
    attempt: u32,
) -> Duration {
    process_retry_backoff_duration(
        match strategy {
            RetryBackoffStrategySpec::Fixed => ProcessRetryBackoffStrategy::Fixed,
            RetryBackoffStrategySpec::Exponential => ProcessRetryBackoffStrategy::Exponential,
        },
        base_backoff_ms,
        attempt,
    )
}

pub(crate) fn command_output_with_timeout(
    command: &mut Command,
    execution: &ImageExecutionContext,
    timeout: Duration,
    label: &str,
    retention: ProcessOutputRetention,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Output, ImageProviderError> {
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
    .map_err(move |error| {
        let kind = match error.kind {
            ProcessRunErrorKind::ToolStart => ImageProviderErrorKind::ToolStart,
            ProcessRunErrorKind::Timeout => ImageProviderErrorKind::Timeout,
            ProcessRunErrorKind::Cancelled => ImageProviderErrorKind::Cancelled,
            ProcessRunErrorKind::RuntimeState => ImageProviderErrorKind::RuntimeState,
        };
        ImageProviderError::new(kind, error.message)
    })
}

pub(crate) struct CommandStdoutToFileRequest<'a> {
    pub(crate) command: &'a mut Command,
    pub(crate) output_path: &'a Path,
    pub(crate) execution: &'a ImageExecutionContext,
    pub(crate) timeout: Duration,
    pub(crate) label: &'a str,
    pub(crate) retention: ProcessOutputRetention,
    pub(crate) log_sink: Option<ProcessLogSink>,
    pub(crate) cancel_check: Option<ProcessCancelCheck>,
}

pub(crate) fn command_stdout_to_file_with_timeout(
    request: CommandStdoutToFileRequest<'_>,
) -> Result<Output, ImageProviderError> {
    let CommandStdoutToFileRequest {
        command,
        output_path,
        execution,
        timeout,
        label,
        retention,
        log_sink,
        cancel_check,
    } = request;
    let label = label.to_string();
    let wrapped_sink = log_sink.map(|sink| label_process_log_sink(label.clone(), sink));
    let mut exec_command = command_for_execution(command, execution)?;
    run_command_stdout_to_file_with_timeout_and_retention(
        &mut exec_command,
        output_path,
        timeout,
        &label,
        retention,
        wrapped_sink,
        cancel_check,
    )
    .map(|result| result.output)
    .map_err(move |error| {
        let kind = match error.kind {
            ProcessRunErrorKind::ToolStart => ImageProviderErrorKind::ToolStart,
            ProcessRunErrorKind::Timeout => ImageProviderErrorKind::Timeout,
            ProcessRunErrorKind::Cancelled => ImageProviderErrorKind::Cancelled,
            ProcessRunErrorKind::RuntimeState => ImageProviderErrorKind::RuntimeState,
        };
        ImageProviderError::new(kind, error.message)
    })
}

pub(crate) fn execution_context(spec: &ResolvedBuildSpec) -> ImageExecutionContext {
    let workspace_root = PathBuf::from(&spec.workspace.root_dir);
    ImageExecutionContext {
        workspace_root: fs::canonicalize(&workspace_root).unwrap_or(workspace_root),
        docker_image: spec
            .policy
            .execution
            .docker
            .as_ref()
            .map(|docker| docker.image.clone()),
    }
}

pub(crate) fn command_for_execution(
    command: &Command,
    execution: &ImageExecutionContext,
) -> Result<Command, ImageProviderError> {
    let Some(image) = &execution.docker_image else {
        return Ok(gaia_process::clone_command(command));
    };
    if image.trim().is_empty() {
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::PolicyBlocked,
            "docker execution requires a non-empty image",
        ));
    }
    let mut spec =
        DockerRunSpec::discovered_mounts(image.clone(), execution.workspace_root.clone(), command);
    // Cache and external-tree directories reach Buildroot through the
    // environment, which mount discovery does not scan. Without mounting
    // them, a cache outside the workspace would live only inside the --rm
    // container and be lost after every build.
    for (key, value) in command.get_envs() {
        let (Some(key), Some(value)) = (key.to_str(), value) else {
            continue;
        };
        if !matches!(key, "BR2_DL_DIR" | "BR2_CCACHE_DIR" | "BR2_EXTERNAL") {
            continue;
        }
        for part in value.to_string_lossy().split([':', ' ']) {
            let path = Path::new(part);
            if path.is_absolute() && path.is_dir() && !spec.mounts.iter().any(|m| m == path) {
                spec.mounts.push(path.to_path_buf());
            }
        }
    }
    docker_run_command(command, &spec).map_err(|error| {
        ImageProviderError::new(ImageProviderErrorKind::PolicyBlocked, error.to_string())
    })
}

/// Shows a line in the run's live output and the log.
pub(crate) fn log_line(command_context: &ImageCommandContext<'_>, line: String) {
    tracing::info!(provider_domain = "image.buildroot", "{line}");
    if let Some(log_sink) = &command_context.log_sink {
        log_sink(gaia_process::ProcessLogLine {
            stream: gaia_process::ProcessLogStream::Stderr,
            line,
        });
    }
}
