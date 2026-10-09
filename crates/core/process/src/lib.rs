use std::ffi::OsStr;
use std::fs::File;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

const MAX_RETAINED_STREAM_BYTES: usize = 1024 * 1024;
const MAX_RETAINED_STREAM_LINES: usize = 1_000;
const STREAM_READ_BUFFER_BYTES: usize = 8 * 1024;
const MAX_RETAINED_LINE_BYTES: usize = 64 * 1024;
const STREAM_MESSAGE_QUEUE_BOUND: usize = 64;

mod docker;
mod retention;
mod step_time;
mod stream;
mod tar;

pub use docker::{
    DockerRunError, DockerRunSpec, absolute_docker_mount_candidate, discover_docker_mounts,
    docker_build_context_hash, docker_image_build_command, docker_image_id_command,
    docker_local_image_tag, docker_run_command, normalize_docker_mount_path,
};
use retention::{
    StreamDrainState, drain_stream_messages, drain_stream_messages_until_idle,
    wait_for_stream_message,
};
pub use step_time::{STEP_TIME_PREFIX, parse_step_time, step_time_message};
use stream::spawn_stream_reader;
pub use tar::{TarArchiveValidationError, validate_tar_archive_entries};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessLogStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessLogLine {
    pub stream: ProcessLogStream,
    pub line: String,
}

pub type ProcessLogSink = Arc<dyn Fn(ProcessLogLine) + Send + Sync + 'static>;
pub type ProcessCancelCheck = Arc<dyn Fn() -> bool + Send + Sync + 'static>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessRetryBackoffStrategy {
    Fixed,
    Exponential,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessRunErrorKind {
    ToolStart,
    Timeout,
    Cancelled,
    RuntimeState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRunError {
    pub kind: ProcessRunErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRunResult {
    pub output: Output,
    pub stdout_lines: Vec<String>,
    pub stderr_lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Byte tails are retained before lossy line decoding; diagnostic lines are
/// bounded separately so invalid UTF-8 and blank output remain observable.
/// Stream readers consume fixed-size chunks and send through a bounded queue,
/// so child output cannot allocate memory proportional to an unbounded stream.
pub struct ProcessOutputRetention {
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub stdout_lines: usize,
    pub stderr_lines: usize,
}

impl Default for ProcessOutputRetention {
    fn default() -> Self {
        Self {
            stdout_bytes: MAX_RETAINED_STREAM_BYTES,
            stderr_bytes: MAX_RETAINED_STREAM_BYTES,
            stdout_lines: MAX_RETAINED_STREAM_LINES,
            stderr_lines: MAX_RETAINED_STREAM_LINES,
        }
    }
}

#[derive(Debug)]
pub(crate) enum StreamMessage {
    Bytes {
        stream: ProcessLogStream,
        bytes: Vec<u8>,
    },
    Line {
        stream: ProcessLogStream,
        line: String,
    },
    Done {
        stream: ProcessLogStream,
    },
}

pub fn run_command_with_timeout(
    command: &mut Command,
    timeout: Duration,
    label: &str,
    sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<ProcessRunResult, ProcessRunError> {
    run_command_with_timeout_and_retention(
        command,
        timeout,
        label,
        ProcessOutputRetention::default(),
        sink,
        cancel_check,
    )
}

pub fn run_command_with_timeout_and_retention(
    command: &mut Command,
    timeout: Duration,
    label: &str,
    retention: ProcessOutputRetention,
    sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<ProcessRunResult, ProcessRunError> {
    let description = command_description(command);
    tracing::debug!(
        command_label = label,
        command_program = %description.program,
        command_args = ?description.args,
        command_cwd = description.cwd.as_deref().unwrap_or("<inherit>"),
        timeout_seconds = timeout.as_secs(),
        "starting process"
    );
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    configure_process_group(command);
    let mut child = spawn_retrying_busy_executable(command).map_err(|error| {
        tracing::warn!(
            command_label = label,
            command_program = %description.program,
            command_args = ?description.args,
            command_cwd = description.cwd.as_deref().unwrap_or("<inherit>"),
            error = %error,
            "process start failed"
        );
        ProcessRunError {
            kind: ProcessRunErrorKind::ToolStart,
            message: format!("failed to start {label}: {error}"),
        }
    })?;
    let child_id = child.id();
    let process_group = ProcessGroup::for_child(&child, command);
    tracing::debug!(
        command_label = label,
        command_program = %description.program,
        command_args = ?description.args,
        command_cwd = description.cwd.as_deref().unwrap_or("<inherit>"),
        pid = child_id,
        timeout_seconds = timeout.as_secs(),
        "process started"
    );

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_sink = sink.clone();
    let stderr_sink = sink;
    let (tx, rx) = mpsc::sync_channel::<StreamMessage>(STREAM_MESSAGE_QUEUE_BOUND);

    spawn_stream_reader(stdout, ProcessLogStream::Stdout, stdout_sink, tx.clone());
    spawn_stream_reader(stderr, ProcessLogStream::Stderr, stderr_sink, tx.clone());
    drop(tx);

    let mut stream_state = StreamDrainState::with_retention(retention);

    let start = Instant::now();
    let status = loop {
        drain_stream_messages(&rx, &mut stream_state);
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if cancel_check.as_ref().is_some_and(|cancel| cancel()) {
                    terminate_child_tree(&mut child, &process_group);
                    drain_stream_messages_until_idle(
                        &rx,
                        &mut stream_state,
                        Duration::from_millis(250),
                    );
                    tracing::warn!(
                        command_label = label,
                        command_program = %description.program,
                        pid = child_id,
                        elapsed_ms = start.elapsed().as_millis(),
                        "process cancelled"
                    );
                    return Err(ProcessRunError {
                        kind: ProcessRunErrorKind::Cancelled,
                        message: format!("{label} cancelled"),
                    });
                }
                if start.elapsed() >= timeout {
                    terminate_child_tree(&mut child, &process_group);
                    drain_stream_messages_until_idle(
                        &rx,
                        &mut stream_state,
                        Duration::from_millis(250),
                    );
                    tracing::warn!(
                        command_label = label,
                        command_program = %description.program,
                        pid = child_id,
                        timeout_seconds = timeout.as_secs(),
                        elapsed_ms = start.elapsed().as_millis(),
                        "process timed out"
                    );
                    return Err(ProcessRunError {
                        kind: ProcessRunErrorKind::Timeout,
                        message: format!("{label} timed out after {}s", timeout.as_secs()),
                    });
                }
                wait_for_stream_message(&rx, &mut stream_state, Duration::from_millis(10));
            }
            Err(error) => {
                terminate_child_tree(&mut child, &process_group);
                drain_stream_messages_until_idle(
                    &rx,
                    &mut stream_state,
                    Duration::from_millis(250),
                );
                tracing::warn!(
                    command_label = label,
                    command_program = %description.program,
                    pid = child_id,
                    elapsed_ms = start.elapsed().as_millis(),
                    error = %error,
                    "process poll failed"
                );
                return Err(ProcessRunError {
                    kind: ProcessRunErrorKind::RuntimeState,
                    message: format!("failed to poll {label}: {error}"),
                });
            }
        }
    };

    process_group.finished();
    drain_stream_messages_until_idle(&rx, &mut stream_state, Duration::from_millis(500));
    tracing::debug!(
        command_label = label,
        command_program = %description.program,
        pid = child_id,
        elapsed_ms = start.elapsed().as_millis(),
        exit_status = %status,
        success = status.success(),
        stdout_lines = stream_state.stdout.lines.len(),
        stderr_lines = stream_state.stderr.lines.len(),
        "process completed"
    );

    Ok(ProcessRunResult {
        output: Output {
            status,
            stdout: stream_state.stdout.take_bytes(),
            stderr: stream_state.stderr.take_bytes(),
        },
        stdout_lines: stream_state.stdout.take_lines(),
        stderr_lines: stream_state.stderr.take_lines(),
    })
}

pub fn run_command_stdout_to_file_with_timeout_and_retention(
    command: &mut Command,
    output_path: &Path,
    timeout: Duration,
    label: &str,
    retention: ProcessOutputRetention,
    sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<ProcessRunResult, ProcessRunError> {
    let description = command_description(command);
    let stdout = File::create(output_path).map_err(|error| ProcessRunError {
        kind: ProcessRunErrorKind::RuntimeState,
        message: format!(
            "failed to create stdout file for {label} at '{}': {error}",
            output_path.display()
        ),
    })?;
    command.stdout(Stdio::from(stdout)).stderr(Stdio::piped());
    configure_process_group(command);
    let mut child = spawn_retrying_busy_executable(command).map_err(|error| {
        tracing::warn!(
            command_label = label,
            command_program = %description.program,
            command_args = ?description.args,
            command_cwd = description.cwd.as_deref().unwrap_or("<inherit>"),
            error = %error,
            "process start failed"
        );
        ProcessRunError {
            kind: ProcessRunErrorKind::ToolStart,
            message: format!("failed to start {label}: {error}"),
        }
    })?;
    let child_id = child.id();
    let process_group = ProcessGroup::for_child(&child, command);
    tracing::debug!(
        command_label = label,
        command_program = %description.program,
        command_args = ?description.args,
        command_cwd = description.cwd.as_deref().unwrap_or("<inherit>"),
        pid = child_id,
        timeout_seconds = timeout.as_secs(),
        "process started"
    );

    let stderr = child.stderr.take();
    let (tx, rx) = mpsc::sync_channel::<StreamMessage>(STREAM_MESSAGE_QUEUE_BOUND);
    spawn_stream_reader(stderr, ProcessLogStream::Stderr, sink, tx.clone());
    drop(tx);

    let mut stream_state = StreamDrainState::with_retention(retention);
    stream_state.stdout_done = true;

    let start = Instant::now();
    let status = loop {
        drain_stream_messages(&rx, &mut stream_state);
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if cancel_check.as_ref().is_some_and(|cancel| cancel()) {
                    terminate_child_tree(&mut child, &process_group);
                    drain_stream_messages_until_idle(
                        &rx,
                        &mut stream_state,
                        Duration::from_millis(250),
                    );
                    tracing::warn!(
                        command_label = label,
                        command_program = %description.program,
                        pid = child_id,
                        elapsed_ms = start.elapsed().as_millis(),
                        "process cancelled"
                    );
                    return Err(ProcessRunError {
                        kind: ProcessRunErrorKind::Cancelled,
                        message: format!("{label} cancelled"),
                    });
                }
                if start.elapsed() >= timeout {
                    terminate_child_tree(&mut child, &process_group);
                    drain_stream_messages_until_idle(
                        &rx,
                        &mut stream_state,
                        Duration::from_millis(250),
                    );
                    tracing::warn!(
                        command_label = label,
                        command_program = %description.program,
                        pid = child_id,
                        timeout_seconds = timeout.as_secs(),
                        elapsed_ms = start.elapsed().as_millis(),
                        "process timed out"
                    );
                    return Err(ProcessRunError {
                        kind: ProcessRunErrorKind::Timeout,
                        message: format!("{label} timed out after {}s", timeout.as_secs()),
                    });
                }
                wait_for_stream_message(&rx, &mut stream_state, Duration::from_millis(10));
            }
            Err(error) => {
                terminate_child_tree(&mut child, &process_group);
                drain_stream_messages_until_idle(
                    &rx,
                    &mut stream_state,
                    Duration::from_millis(250),
                );
                tracing::warn!(
                    command_label = label,
                    command_program = %description.program,
                    pid = child_id,
                    elapsed_ms = start.elapsed().as_millis(),
                    error = %error,
                    "process poll failed"
                );
                return Err(ProcessRunError {
                    kind: ProcessRunErrorKind::RuntimeState,
                    message: format!("failed to poll {label}: {error}"),
                });
            }
        }
    };

    process_group.finished();
    drain_stream_messages_until_idle(&rx, &mut stream_state, Duration::from_millis(500));

    Ok(ProcessRunResult {
        output: Output {
            status,
            stdout: Vec::new(),
            stderr: stream_state.stderr.take_bytes(),
        },
        stdout_lines: Vec::new(),
        stderr_lines: stream_state.stderr.take_lines(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessCommandDescription {
    program: String,
    args: Vec<String>,
    cwd: Option<String>,
}

fn command_description(command: &Command) -> ProcessCommandDescription {
    ProcessCommandDescription {
        program: os_str_lossy(command.get_program()),
        args: sanitized_args(command.get_args().map(os_str_lossy)),
        cwd: command
            .get_current_dir()
            .map(|path| path.display().to_string()),
    }
}

fn sanitized_args(args: impl Iterator<Item = String>) -> Vec<String> {
    let mut redact_next = false;
    args.map(|arg| {
        if redact_next {
            redact_next = false;
            return "<redacted>".to_string();
        }
        if sensitive_flag(&arg) && !arg.contains('=') {
            redact_next = true;
            return arg;
        }
        sanitize_arg(&arg)
    })
    .collect()
}

fn sanitize_arg(arg: &str) -> String {
    if let Some((prefix, rest)) = arg.split_once("://")
        && let Some((userinfo, host_and_path)) = rest.split_once('@')
        && (userinfo.contains(':') || sensitive_name(userinfo))
    {
        return redact_sensitive_query_values(&format!("{prefix}://<redacted>@{host_and_path}"));
    }
    if arg.contains('?') {
        return redact_sensitive_query_values(arg);
    }
    if let Some((key, _)) = arg.split_once('=')
        && sensitive_name(key)
    {
        return format!("{key}=<redacted>");
    }
    arg.to_string()
}

fn redact_sensitive_query_values(arg: &str) -> String {
    let Some((base, query)) = arg.split_once('?') else {
        return arg.to_string();
    };
    let redacted = query
        .split('&')
        .map(|part| {
            part.split_once('=').map_or_else(
                || part.to_string(),
                |(key, value)| {
                    if sensitive_name(key) {
                        format!("{key}=<redacted>")
                    } else {
                        format!("{key}={value}")
                    }
                },
            )
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}?{redacted}")
}

fn sensitive_flag(arg: &str) -> bool {
    sensitive_name(arg.trim_start_matches('-'))
}

fn sensitive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "token",
        "password",
        "passwd",
        "secret",
        "apikey",
        "api_key",
        "credential",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn os_str_lossy(value: &OsStr) -> String {
    value.to_string_lossy().into_owned()
}

pub fn sleep_with_cancel(duration: Duration, cancel_check: Option<&ProcessCancelCheck>) -> bool {
    let Some(deadline) = Instant::now().checked_add(duration) else {
        return false;
    };
    loop {
        if cancel_check.is_some_and(|cancel| cancel()) {
            return false;
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return true;
        };
        thread::sleep(remaining.min(Duration::from_millis(50)));
    }
}

pub fn retry_backoff_duration(
    strategy: ProcessRetryBackoffStrategy,
    base_backoff_ms: u64,
    attempt: u32,
) -> Duration {
    let millis = match strategy {
        ProcessRetryBackoffStrategy::Fixed => base_backoff_ms,
        ProcessRetryBackoffStrategy::Exponential => {
            let exponent = attempt.saturating_sub(1).min(20);
            base_backoff_ms.saturating_mul(1u64 << exponent)
        }
    };
    Duration::from_millis(millis)
}

pub fn clone_command(command: &Command) -> Command {
    let mut cloned = Command::new(command.get_program());
    cloned.args(command.get_args());
    if let Some(current_dir) = command.get_current_dir() {
        cloned.current_dir(current_dir);
    }
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => {
                cloned.env(key, value);
            }
            None => {
                cloned.env_remove(key);
            }
        }
    }
    cloned
}

pub fn label_process_log_sink(label: impl Into<String>, sink: ProcessLogSink) -> ProcessLogSink {
    let label = label.into();
    Arc::new(move |line: ProcessLogLine| {
        sink(ProcessLogLine {
            stream: line.stream,
            line: format!(
                "{label} {}: {}",
                match line.stream {
                    ProcessLogStream::Stdout => "stdout",
                    ProcessLogStream::Stderr => "stderr",
                },
                line.line
            ),
        });
    })
}

pub(crate) fn output_text(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        stderr
    }
}

/// Spawns `command`, retrying briefly when the executable is "busy".
///
/// `ETXTBSY` happens when a freshly written executable is still open for
/// writing in a child that another thread forked but has not exec'd yet (the
/// child briefly inherits the descriptor). It clears within milliseconds, so
/// a few short retries make freshly generated scripts safe to run.
fn spawn_retrying_busy_executable(command: &mut Command) -> std::io::Result<Child> {
    const ATTEMPTS: u32 = 20;
    let mut attempt = 1;
    loop {
        match command.spawn() {
            Err(error)
                if error.kind() == std::io::ErrorKind::ExecutableFileBusy && attempt < ATTEMPTS =>
            {
                thread::sleep(Duration::from_millis(5 * u64::from(attempt)));
                attempt += 1;
            }
            result => return result,
        }
    }
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

/// What to stop when a command ends or is stopped early: its process group
/// and, for `docker run`, the container, which lives outside that group.
#[derive(Debug, Clone)]
struct ProcessGroup {
    #[cfg(unix)]
    pgid: libc::pid_t,
    container: Option<docker::DockerContainer>,
}

impl ProcessGroup {
    fn for_child(child: &Child, command: &Command) -> Self {
        Self {
            #[cfg(unix)]
            pgid: child.id() as libc::pid_t,
            container: docker::DockerContainer::for_command(command),
        }
    }

    /// The command exited on its own; its container (`--rm`) is gone too.
    fn finished(&self) {
        terminate_leftover_tree(self);
        if let Some(container) = &self.container {
            container.discard_id_file();
        }
    }
}

/// Stops a command early (timeout, cancellation, failure). Killing the
/// docker client does not stop its container, so the container is removed
/// explicitly; otherwise it keeps running with nobody reading its output.
fn terminate_child_tree(child: &mut Child, group: &ProcessGroup) {
    terminate_leftover_tree(group);
    let _ = child.kill();
    let _ = child.wait();
    if let Some(container) = &group.container {
        container.remove();
    }
}

#[cfg(unix)]
fn terminate_leftover_tree(group: &ProcessGroup) {
    let process_group_id = -group.pgid;
    // SAFETY: `kill` is called with a negative process-group id obtained from
    // a child this process spawned with `process_group(0)`. Errors are ignored
    // because the group may already be empty by the time cleanup runs.
    unsafe {
        libc::kill(process_group_id, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn terminate_leftover_tree(_group: &ProcessGroup) {}

#[cfg(all(test, unix))]
mod docker_tests;
#[cfg(test)]
mod tests;
