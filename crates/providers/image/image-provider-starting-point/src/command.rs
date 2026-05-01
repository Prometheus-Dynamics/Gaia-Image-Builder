use super::*;

pub(crate) fn stderr_or_stdout(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        stderr
    } else {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }
}

pub(crate) fn command_status(
    command: &mut Command,
    label: &str,
    error_kind: ImageProviderErrorKind,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(), ImageProviderError> {
    let output = command_output(command, label, policy, log_sink, cancel_check)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(ImageProviderError::new(
            error_kind,
            format!("{label} failed: {}", stderr_or_stdout(&output)),
        ))
    }
}

pub(crate) fn cleanup_command_status(
    command: &mut Command,
    label: &str,
    error_kind: ImageProviderErrorKind,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(), ImageProviderError> {
    let timeout = Duration::from_secs(
        policy
            .timeout_seconds
            .clamp(1, PRIVILEGED_CLEANUP_TIMEOUT_SECONDS),
    );
    let output = command_output_with_duration(
        command,
        label,
        timeout,
        policy.output_retention,
        log_sink,
        cancel_check,
    )?;
    if output.status.success() {
        Ok(())
    } else {
        Err(ImageProviderError::new(
            error_kind,
            format!("{label} failed: {}", stderr_or_stdout(&output)),
        ))
    }
}

pub(crate) fn command_output(
    command: &mut Command,
    label: &str,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Output, ImageProviderError> {
    command_output_with_duration(
        command,
        label,
        Duration::from_secs(policy.timeout_seconds.max(1)),
        policy.output_retention,
        log_sink,
        cancel_check,
    )
}

pub(crate) fn command_output_with_duration(
    command: &mut Command,
    label: &str,
    timeout: Duration,
    retention: ProcessOutputRetention,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Output, ImageProviderError> {
    run_command_with_timeout_and_retention(
        command,
        timeout,
        label,
        retention,
        log_sink,
        cancel_check,
    )
    .map(|result| result.output)
    .map_err(|error| {
        let kind = match error.kind {
            ProcessRunErrorKind::ToolStart => ImageProviderErrorKind::ToolStart,
            ProcessRunErrorKind::Timeout => ImageProviderErrorKind::Timeout,
            ProcessRunErrorKind::Cancelled => ImageProviderErrorKind::Cancelled,
            ProcessRunErrorKind::RuntimeState => ImageProviderErrorKind::RuntimeState,
        };
        ImageProviderError::new(kind, error.message)
    })
}

pub(crate) fn remove_path_if_exists(path: &Path) -> Result<(), ImageProviderError> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        fs::remove_dir_all(path).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to remove directory '{}': {error}",
                path.display()
            ))
        })
    } else {
        fs::remove_file(path).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to remove file '{}': {error}",
                path.display()
            ))
        })
    }
}

pub(crate) fn create_rootfs_archive(
    rootfs: &Path,
    archive_path: &Path,
    execution: &ImageExecutionContext,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Vec<String>, ImageProviderError> {
    if let Some(parent) = archive_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to create starting-point archive dir '{}': {error}",
                parent.display()
            ))
        })?;
    }

    let mut command = Command::new("tar");
    command.arg("-cf").arg(archive_path);
    if rootfs.is_dir() {
        let parent = rootfs.parent().unwrap_or_else(|| Path::new("/"));
        let name = rootfs
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("rootfs");
        command.arg("-C").arg(parent).arg(name);
    } else {
        let parent = rootfs.parent().unwrap_or_else(|| Path::new("/"));
        let name = rootfs
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("rootfs");
        command.arg("-C").arg(parent).arg(name);
    }
    run_command(
        command,
        archive_path,
        execution,
        policy,
        log_sink,
        cancel_check,
    )
}

pub(crate) fn run_command(
    mut command: Command,
    archive_path: &Path,
    execution: &ImageExecutionContext,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Vec<String>, ImageProviderError> {
    let attempts = policy.retry_attempts.max(1);
    let timeout = Duration::from_secs(policy.timeout_seconds.max(1));
    let mut last_error = String::new();
    let label = format!("starting-point archive build '{}'", archive_path.display());
    for attempt in 1..=attempts {
        tracing::debug!(
            command_label = label.as_str(),
            provider_domain = "image.starting-point",
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
            &label,
            log_sink.clone(),
            cancel_check.clone(),
        )?;
        if output.status.success() {
            tracing::debug!(
                command_label = label.as_str(),
                provider_domain = "image.starting-point",
                backend = execution_backend(execution),
                attempt,
                attempts,
                "image provider command succeeded"
            );
            return Ok(Vec::new());
        }
        last_error = format!(
            "starting-point archive build failed for '{}' on attempt {attempt}/{attempts}: {}",
            archive_path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
        if attempt < attempts {
            let retry_backoff = retry_backoff_duration(
                policy.retry_backoff_strategy,
                policy.retry_backoff_ms,
                attempt,
            );
            tracing::warn!(
                command_label = label.as_str(),
                provider_domain = "image.starting-point",
                backend = execution_backend(execution),
                attempt,
                attempts,
                backoff_ms = retry_backoff.as_millis(),
                "image provider command failed; retrying"
            );
            if !sleep_with_cancel(retry_backoff, cancel_check.as_ref()) {
                tracing::warn!(
                    command_label = label.as_str(),
                    provider_domain = "image.starting-point",
                    backend = execution_backend(execution),
                    attempt,
                    attempts,
                    "image provider retry backoff cancelled"
                );
                return Err(ImageProviderError::new(
                    ImageProviderErrorKind::Cancelled,
                    format!(
                        "starting-point archive build '{}' cancelled during retry backoff",
                        archive_path.display()
                    ),
                ));
            }
        }
    }
    tracing::warn!(
        command_label = label.as_str(),
        provider_domain = "image.starting-point",
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
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Output, ImageProviderError> {
    let label = label.to_string();
    let wrapped_sink = log_sink.map(|sink| label_process_log_sink(label.clone(), sink));
    let mut exec_command = command_for_execution(command, execution)?;
    run_command_with_timeout(
        &mut exec_command,
        timeout,
        &label,
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
    let spec =
        DockerRunSpec::discovered_mounts(image.clone(), execution.workspace_root.clone(), command);
    docker_run_command(command, &spec).map_err(|error| {
        ImageProviderError::new(ImageProviderErrorKind::PolicyBlocked, error.to_string())
    })
}

pub(crate) fn copy_dir(src: &Path, dest: &Path) -> Result<(), ImageProviderError> {
    fs::create_dir_all(dest).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to create starting-point rootfs dir '{}': {error}",
            dest.display()
        ))
    })?;
    for entry in fs::read_dir(src).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to read starting-point rootfs '{}': {error}",
            src.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to read starting-point rootfs entry '{}': {error}",
                src.display()
            ))
        })?;
        let path = entry.path();
        let target = dest.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &target)?;
        } else {
            fs::copy(&path, &target).map_err(|error| {
                ImageProviderError::backend_command(format!(
                    "failed to copy starting-point rootfs file '{}' to '{}': {error}",
                    path.display(),
                    target.display()
                ))
            })?;
        }
    }
    Ok(())
}
