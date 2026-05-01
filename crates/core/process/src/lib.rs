use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

const MAX_RETAINED_STREAM_BYTES: usize = 1024 * 1024;
const MAX_RETAINED_STREAM_LINES: usize = 1_000;

mod docker;
mod tar;

pub use docker::{
    DockerRunError, DockerRunSpec, absolute_docker_mount_candidate, discover_docker_mounts,
    docker_run_command, normalize_docker_mount_path,
};
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
enum StreamMessage {
    Data {
        stream: ProcessLogStream,
        line: String,
        bytes: Vec<u8>,
    },
    Done {
        stream: ProcessLogStream,
    },
}

#[derive(Debug)]
struct RetainedStream {
    lines: Vec<String>,
    bytes: Vec<u8>,
    max_lines: usize,
    max_bytes: usize,
}

impl Default for RetainedStream {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            bytes: Vec::new(),
            max_lines: MAX_RETAINED_STREAM_LINES,
            max_bytes: MAX_RETAINED_STREAM_BYTES,
        }
    }
}

impl RetainedStream {
    fn with_limits(max_lines: usize, max_bytes: usize) -> Self {
        Self {
            lines: Vec::new(),
            bytes: Vec::new(),
            max_lines,
            max_bytes,
        }
    }

    fn push(&mut self, line: String, bytes: &[u8]) {
        if self.max_lines > 0 {
            self.lines.push(line);
            if self.lines.len() > self.max_lines {
                let excess = self.lines.len() - self.max_lines;
                self.lines.drain(0..excess);
            }
        }

        if self.max_bytes > 0 {
            self.bytes.extend_from_slice(bytes);
            if self.bytes.len() > self.max_bytes {
                let excess = self.bytes.len() - self.max_bytes;
                self.bytes.drain(0..excess);
            }
        }
    }
}

#[derive(Debug, Default)]
struct StreamDrainState {
    stdout: RetainedStream,
    stderr: RetainedStream,
    stdout_done: bool,
    stderr_done: bool,
}

impl StreamDrainState {
    fn with_retention(retention: ProcessOutputRetention) -> Self {
        Self {
            stdout: RetainedStream::with_limits(retention.stdout_lines, retention.stdout_bytes),
            stderr: RetainedStream::with_limits(retention.stderr_lines, retention.stderr_bytes),
            stdout_done: false,
            stderr_done: false,
        }
    }

    fn is_done(&self) -> bool {
        self.stdout_done && self.stderr_done
    }
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
    let mut child = command.spawn().map_err(|error| {
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
    let process_group = ProcessGroup::for_child(&child);
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
    let (tx, rx) = mpsc::channel::<StreamMessage>();

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
                    terminate_child_tree(&mut child, process_group);
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
                    terminate_child_tree(&mut child, process_group);
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
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                terminate_child_tree(&mut child, process_group);
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

    terminate_leftover_tree(process_group);
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
            stdout: stream_state.stdout.bytes,
            stderr: stream_state.stderr.bytes,
        },
        stdout_lines: stream_state.stdout.lines,
        stderr_lines: stream_state.stderr.lines,
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

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

#[derive(Debug, Clone, Copy)]
struct ProcessGroup {
    #[cfg(unix)]
    pgid: libc::pid_t,
}

impl ProcessGroup {
    fn for_child(child: &Child) -> Self {
        Self {
            #[cfg(unix)]
            pgid: child.id() as libc::pid_t,
        }
    }
}

fn terminate_child_tree(child: &mut Child, group: ProcessGroup) {
    terminate_leftover_tree(group);
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn terminate_leftover_tree(group: ProcessGroup) {
    let process_group_id = -group.pgid;
    // SAFETY: `kill` is called with a negative process-group id obtained from
    // a child this process spawned with `process_group(0)`. Errors are ignored
    // because the group may already be empty by the time cleanup runs.
    unsafe {
        libc::kill(process_group_id, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn terminate_leftover_tree(_group: ProcessGroup) {}

fn spawn_stream_reader(
    stream: Option<impl Read + Send + 'static>,
    which: ProcessLogStream,
    sink: Option<ProcessLogSink>,
    tx: Sender<StreamMessage>,
) {
    thread::spawn(move || {
        read_stream(stream, which, sink, tx);
    });
}

fn read_stream(
    stream: Option<impl Read + Send + 'static>,
    which: ProcessLogStream,
    sink: Option<ProcessLogSink>,
    tx: Sender<StreamMessage>,
) {
    let Some(stream) = stream else {
        let _ = tx.send(StreamMessage::Done { stream: which });
        return;
    };
    let mut reader = BufReader::new(stream);
    let mut buffer = String::new();
    loop {
        buffer.clear();
        match reader.read_line(&mut buffer) {
            Ok(0) => break,
            Ok(_) => {
                let bytes = buffer.as_bytes().to_vec();
                let line = buffer.trim_end_matches(['\r', '\n']).to_string();
                if line.is_empty() {
                    continue;
                }
                if let Some(sink) = &sink {
                    sink(ProcessLogLine {
                        stream: which,
                        line: line.clone(),
                    });
                }
                if tx
                    .send(StreamMessage::Data {
                        stream: which,
                        line,
                        bytes,
                    })
                    .is_err()
                {
                    return;
                }
            }
            Err(_) => break,
        }
    }
    let _ = tx.send(StreamMessage::Done { stream: which });
}

fn drain_stream_messages(rx: &Receiver<StreamMessage>, state: &mut StreamDrainState) {
    while let Ok(message) = rx.try_recv() {
        apply_stream_message(message, state);
    }
}

fn drain_stream_messages_until_idle(
    rx: &Receiver<StreamMessage>,
    state: &mut StreamDrainState,
    idle_timeout: Duration,
) {
    let deadline = Instant::now() + idle_timeout;
    while !state.is_done() {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        match rx.recv_timeout(remaining) {
            Ok(message) => apply_stream_message(message, state),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
        }
    }
    drain_stream_messages(rx, state);
}

fn apply_stream_message(message: StreamMessage, state: &mut StreamDrainState) {
    match message {
        StreamMessage::Data {
            stream: ProcessLogStream::Stdout,
            line,
            bytes,
        } => {
            state.stdout.push(line, &bytes);
        }
        StreamMessage::Data {
            stream: ProcessLogStream::Stderr,
            line,
            bytes,
        } => {
            state.stderr.push(line, &bytes);
        }
        StreamMessage::Done {
            stream: ProcessLogStream::Stdout,
        } => state.stdout_done = true,
        StreamMessage::Done {
            stream: ProcessLogStream::Stderr,
        } => state.stderr_done = true,
    }
}

#[cfg(test)]
mod tests;
