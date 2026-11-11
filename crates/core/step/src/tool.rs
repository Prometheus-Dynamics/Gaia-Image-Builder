//! Running one tool process.

use crate::error::ErrorKind;
use crate::{Ctx, StepError};
use std::collections::VecDeque;
use std::ffi::OsString;
use std::fmt;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Lines of each stream kept for the failure report.
pub const TAIL_LINES: usize = 40;
/// A longer line is cut to this many bytes.
const LINE_BYTES: usize = 1024;
/// How often a running tool is checked for exit and cancel.
const POLL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

type LineFn = dyn Fn(Stream, &str) + Send + Sync;
type LineSink = Box<LineFn>;

/// One tool invocation: a program, its arguments and its whole environment.
/// Nothing is inherited from the process that runs it; a tool that needs
/// `PATH` or `HOME` is given them.
pub struct Tool {
    program: PathBuf,
    args: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
    cwd: Option<PathBuf>,
    on_line: Option<LineSink>,
}

impl fmt::Debug for Tool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tool")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("env", &self.env)
            .field("cwd", &self.cwd)
            .finish_non_exhaustive()
    }
}

impl Tool {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: None,
            on_line: None,
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I: IntoIterator<Item = A>, A: Into<OsString>>(mut self, args: I) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Where the tool starts. The step's output directory when not set.
    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    /// Called with every line the tool prints, as it prints it.
    pub fn on_line(mut self, sink: impl Fn(Stream, &str) + Send + Sync + 'static) -> Self {
        self.on_line = Some(Box::new(sink));
        self
    }
}

/// What a tool that exited 0 printed last.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolOutput {
    pub stdout_tail: Vec<String>,
    pub stderr_tail: Vec<String>,
}

pub(crate) fn run(tool: &Tool, cx: &Ctx, step: &str, dir: &Path) -> Result<ToolOutput, StepError> {
    let mut command = Command::new(&tool.program);
    command
        .args(&tool.args)
        .env_clear()
        .envs(tool.env.iter().map(|(key, value)| (key, value)))
        .current_dir(tool.cwd.as_deref().unwrap_or(dir))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Its own process group, so cancel reaches everything it starts.
        .process_group(0);
    let mut child = command.spawn().map_err(|error| {
        let why = format!("{}: {error}", tool.program.display());
        StepError::new(step, ErrorKind::Spawn(why), Vec::new())
    })?;
    let group = child.id();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let sink = tool.on_line.as_deref();

    std::thread::scope(|scope| {
        let out = scope.spawn(move || tail(stdout, Stream::Stdout, sink));
        let err = scope.spawn(move || tail(stderr, Stream::Stderr, sink));
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if cx.cancelled() => break None,
                Ok(None) => std::thread::sleep(POLL),
                Err(_) => break child.wait().ok(),
            }
        };
        // Whatever the tool left running in its group goes with it, so no
        // stray process outlives the step or holds its pipes open.
        kill_group(group);
        let _ = child.wait();
        let stdout_tail = out.join().unwrap_or_default();
        let stderr_tail = err.join().unwrap_or_default();
        let kind = match status {
            None => ErrorKind::Cancelled,
            Some(status) if status.success() => {
                return Ok(ToolOutput {
                    stdout_tail,
                    stderr_tail,
                });
            }
            Some(status) => match (status.code(), status.signal()) {
                (Some(code), _) => ErrorKind::Exit(code),
                (None, Some(signal)) => ErrorKind::Signal(signal),
                (None, None) => ErrorKind::Exit(-1),
            },
        };
        Err(StepError::new(step, kind, stderr_tail))
    })
}

fn kill_group(group: u32) {
    let Some(pid) = i32::try_from(group)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return;
    };
    let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
}

/// Reads a stream to its end, handing each line to `sink` and keeping the
/// last [`TAIL_LINES`].
fn tail(stream: Option<impl Read>, which: Stream, sink: Option<&LineFn>) -> Vec<String> {
    let Some(stream) = stream else {
        return Vec::new();
    };
    let mut reader = BufReader::new(stream);
    let mut lines = VecDeque::with_capacity(TAIL_LINES);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        while buffer
            .last()
            .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
        {
            buffer.pop();
        }
        let mut line = String::from_utf8_lossy(&buffer).into_owned();
        if line.len() > LINE_BYTES {
            let mut end = LINE_BYTES;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            line.truncate(end);
        }
        if let Some(sink) = sink {
            sink(which, &line);
        }
        if lines.len() == TAIL_LINES {
            lines.pop_front();
        }
        lines.push_back(line);
    }
    lines.into()
}
