use super::*;
use gaia_image_providers::{AttemptFailure, RetryRun, run_with_retries};

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

/// A timeout no build reaches, for a `make` with `make_timeout_seconds = 0`.
const NO_TIMEOUT: Duration = Duration::from_secs(10 * 365 * 24 * 60 * 60);

/// Stderr fragments that name a download or network failure. Only these make
/// a failed `make` worth another attempt: a compile error fails the same way
/// again.
const NETWORK_FAILURE_MARKERS: &[&str] = &[
    "failed to download",
    "download failed",
    "failed to fetch",
    "spurious network error",
    "could not resolve host",
    "unable to resolve host",
    "temporary failure in name resolution",
    "connection timed out",
    "connection reset",
    "connection refused",
    "network is unreachable",
    "curl: (",
    "wget: ",
];

/// Whether a failed command's stderr reports a download or network failure.
pub(crate) fn looks_like_network_failure(stderr: &[u8]) -> bool {
    let text = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    NETWORK_FAILURE_MARKERS
        .iter()
        .any(|marker| text.contains(marker))
}

/// The policy the long `make` that builds the image runs with: its own
/// timeout and attempts. The other Buildroot commands keep the provider's.
pub(crate) fn make_policy(policy: &ImageExecutionPolicy) -> ImageExecutionPolicy {
    ImageExecutionPolicy {
        timeout_seconds: policy.make_timeout_seconds,
        retry_attempts: policy.make_retry_attempts,
        ..policy.clone()
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
    let is_make = command.get_program() == "make";
    let timeout = if policy.timeout_seconds == 0 {
        NO_TIMEOUT
    } else {
        Duration::from_secs(policy.timeout_seconds.max(1))
    };
    let attempts = policy.retry_attempts.max(1);
    let run = RetryRun {
        domain: "image.buildroot",
        label,
        backend: execution_backend(execution),
        docker_image: execution.docker_image.as_deref(),
        timeout,
        attempts,
        backoff_strategy: policy.retry_backoff_strategy,
        backoff_ms: policy.retry_backoff_ms,
        cancel_check: cancel_check.as_ref(),
    };
    run_with_retries(&run, |attempt| {
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
            return Ok(None);
        }
        // A make that failed for any other reason (a compile error) fails the
        // same way again: it is not retried.
        Ok(Some(AttemptFailure {
            message: format!(
                "{label} failed on attempt {attempt}/{attempts}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            retryable: !is_make || looks_like_network_failure(&output.stderr),
        }))
    })
}

fn execution_backend(execution: &ImageExecutionContext) -> &'static str {
    if execution.docker_image.is_some() {
        "docker"
    } else {
        "host"
    }
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
    // Cache and external-tree directories, and the output tree's own
    // directories (for commands run without `O=`, such as filesystem image
    // scripts), reach commands through the environment, which mount
    // discovery does not scan. Without mounting them, a cache outside the
    // workspace would live only inside the --rm container and be lost after
    // every build, and an output tree outside it (a RAM tree) would be
    // missing.
    for (key, value) in command.get_envs() {
        let (Some(key), Some(value)) = (key.to_str(), value) else {
            continue;
        };
        if !matches!(
            key,
            "BR2_DL_DIR"
                | "BR2_CCACHE_DIR"
                | "BR2_EXTERNAL"
                | "BASE_DIR"
                | "BUILD_DIR"
                | "BINARIES_DIR"
                | "TARGET_DIR"
                | "HOST_DIR"
                | "STAGING_DIR"
                | "BR2_CONFIG"
                | "CCACHE_STATSLOG"
        ) {
            continue;
        }
        for part in value.to_string_lossy().split([':', ' ']) {
            let path = Path::new(part);
            if !path.is_absolute() {
                continue;
            }
            // A file (a config, a log to write) is reached through its
            // directory.
            let dir = if path.is_dir() {
                Some(path)
            } else {
                path.parent()
            };
            if let Some(dir) = dir.filter(|dir| dir.is_dir())
                && !spec.mounts.iter().any(|mount| dir.starts_with(mount))
            {
                spec.mounts.push(dir.to_path_buf());
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
