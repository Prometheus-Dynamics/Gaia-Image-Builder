//! Running one tool process.

use crate::error::ErrorKind;
use crate::log::RunLog;
use crate::{Ctx, StepError};
use gaia_store::StepKey;
use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
use std::collections::VecDeque;
use std::ffi::OsString;
use std::fmt;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
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
    /// Lines that fail the run as soon as the tool prints them, and why.
    refused: Vec<(String, String)>,
    /// A line printed this many times in a row on one stream fails the
    /// run, with why.
    repeated: Option<(usize, String)>,
    /// The niceness the tool runs at, when not the step's own.
    nice: Option<i32>,
}

impl fmt::Debug for Tool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tool")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("env", &self.env)
            .field("cwd", &self.cwd)
            .field("refused", &self.refused)
            .field("repeated", &self.repeated)
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
            refused: Vec::new(),
            repeated: None,
            nice: None,
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

    /// Fails the run as soon as the tool prints a line holding `text`: its
    /// process group is killed, and the error gives `why` and the line. For
    /// a warning that means the run, if it went on, would make the wrong
    /// thing or make it the wrong way.
    pub fn refuse_line(mut self, text: impl Into<String>, why: impl Into<String>) -> Self {
        self.refused.push((text.into(), why.into()));
        self
    }

    /// Fails the run as soon as the tool prints the same line `times` times
    /// in a row on one stream: its process group is killed, and the error
    /// gives `why` and the line. For a tool that, stuck, repeats itself
    /// without end (a configuration prompt read from no terminal asks
    /// again for ever), filling the run log instead of failing.
    pub fn refuse_repeated(mut self, times: usize, why: impl Into<String>) -> Self {
        self.repeated = Some((times.max(2), why.into()));
        self
    }

    /// Runs the tool, and everything it starts, at niceness `nice` (19 is
    /// the lowest priority): it gets the CPU the processes at the normal
    /// priority leave idle. The tool is started through `nice`, so it runs
    /// at that niceness from its first instruction and what it starts
    /// inherits it (where the machine has no `nice`, it is set on the
    /// tool's process group as soon as it is started). It changes how fast
    /// the tool runs, never what it makes.
    pub fn nice(mut self, nice: i32) -> Self {
        self.nice = Some(nice);
        self
    }
}

/// What a tool that exited 0 printed last, and its run log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolOutput {
    pub stdout_tail: Vec<String>,
    pub stderr_tail: Vec<String>,
    pub log: PathBuf,
}

impl Tool {
    /// The program and its arguments, as one line for the run log.
    fn command_line(&self) -> String {
        std::iter::once(self.program.as_os_str())
            .chain(self.args.iter().map(OsString::as_os_str))
            .map(|part| part.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The machine's `nice` (coreutils), by its path.
fn nice_program() -> Option<&'static Path> {
    ["/usr/bin/nice", "/bin/nice"]
        .into_iter()
        .map(Path::new)
        .find(|path| path.is_file())
}

/// Runs `tool` to its end and adds the CPU time its process and the
/// processes it waited for took, in nanoseconds, to `cpu_ns`. The time is
/// read before the child is reaped, so it is also read when the run fails.
pub(crate) fn run(
    tool: &Tool,
    cx: &Ctx,
    step: &str,
    dir: &Path,
    logs: &Path,
    key: &StepKey,
    cpu_ns: &mut u64,
) -> Result<ToolOutput, StepError> {
    let log = RunLog::create(logs, step, key, &tool.command_line()).map_err(|error| {
        let why = format!("cannot write a run log in {}: {error}", logs.display());
        StepError::new(step, ErrorKind::Body(why), Vec::new())
    })?;
    // `nice` adds to the niceness it has, Gaia's: by how much more the tool
    // is to have. Lowering one's own priority is always allowed.
    let own = rustix::process::getpriority_process(None).unwrap_or(0);
    let through_nice = tool
        .nice
        .filter(|&nice| nice > own)
        .and_then(|nice| nice_program().map(|program| (program, nice - own)));
    let mut command = match &through_nice {
        Some((program, more)) => {
            let mut command = Command::new(program);
            command.arg("-n").arg(more.to_string()).arg(&tool.program);
            command
        }
        None => Command::new(&tool.program),
    };
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
        log.end(&format!("could not start: {why}"));
        StepError::new(step, ErrorKind::Spawn(why), Vec::new()).with_log(log.path.clone())
    })?;
    let group = child.id();
    if let (Some(nice), Some(pid), None) = (tool.nice, pid_of(group), &through_nice) {
        // Lowering one's own child's priority is always allowed; were it
        // refused, the tool would run at the step's priority, as before.
        let _ = rustix::process::setpriority_pgrp(Some(pid), nice);
    }
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let sink = tool.on_line.as_deref();
    let watch = Watch {
        refused: &tool.refused,
        repeated: tool.repeated.as_ref(),
        seen: OnceLock::new(),
    };

    std::thread::scope(|scope| {
        let (log, watch) = (&log, &watch);
        let out = scope.spawn(move || tail(stdout, Stream::Stdout, sink, watch, log));
        let err = scope.spawn(move || tail(stderr, Stream::Stderr, sink, watch, log));
        // Waits for the exit without reaping it: the child stays a zombie
        // until its CPU time is read below.
        let cancelled = loop {
            match exited(group) {
                Ok(true) => break false,
                Ok(false) if cx.cancelled() || watch.seen.get().is_some() => break true,
                Ok(false) => std::thread::sleep(POLL),
                Err(_) => break false,
            }
        };
        // Whatever the tool left running in its group goes with it, so no
        // stray process outlives the step or holds its pipes open.
        kill_group(group);
        if let Some(pid) = pid_of(group) {
            let _ = waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOWAIT,
            );
        }
        *cpu_ns += cpu_time_ns(group).unwrap_or(0);
        let status = child.wait().ok().filter(|_| !cancelled);
        let stdout_tail = out.join().unwrap_or_default();
        let stderr_tail = err.join().unwrap_or_default();
        let kind = match (watch.seen.get(), status) {
            (Some(refused), _) => ErrorKind::Body(refused.clone()),
            (None, status) => match status {
                None => ErrorKind::Cancelled,
                Some(status) if status.success() => {
                    log.end("ok");
                    return Ok(ToolOutput {
                        stdout_tail,
                        stderr_tail,
                        log: log.path.clone(),
                    });
                }
                Some(status) => match (status.code(), status.signal()) {
                    (Some(code), _) => ErrorKind::Exit(code),
                    (None, Some(signal)) => ErrorKind::Signal(signal),
                    (None, None) => ErrorKind::Exit(-1),
                },
            },
        };
        log.end(
            match &kind {
                ErrorKind::Exit(code) => format!("exit {code}"),
                ErrorKind::Signal(signal) => format!("signal {signal}"),
                ErrorKind::Body(why) => format!("refused: {why}"),
                _ => "cancelled".to_string(),
            }
            .as_str(),
        );
        Err(StepError::new(step, kind, stderr_tail).with_log(log.path.clone()))
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

/// The process ID of a child, as the wait calls take it.
fn pid_of(group: u32) -> Option<Pid> {
    i32::try_from(group).ok().and_then(Pid::from_raw)
}

/// Whether the child `group` has exited, without reaping it. A child whose
/// ID cannot be a process ID has nothing to wait for.
fn exited(group: u32) -> std::io::Result<bool> {
    let Some(pid) = pid_of(group) else {
        return Ok(true);
    };
    let status = waitid(
        WaitId::Pid(pid),
        WaitIdOptions::EXITED | WaitIdOptions::NOWAIT | WaitIdOptions::NOHANG,
    )?;
    Ok(status.is_some())
}

/// Clock ticks per second in `/proc`'s CPU fields. Linux reports them in
/// `USER_HZ`, which is 100 on every architecture Gaia builds for.
const USER_HZ: u64 = 100;

/// User plus system CPU time of the zombie `pid`, its waited-for children
/// included (`/proc/<pid>/stat` fields 14 to 17). `None` when the file
/// cannot be read or parsed.
fn cpu_time_ns(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is in parentheses and may hold spaces; the fields
    // after its last parenthesis start at field 3, the state.
    let after = stat.rsplit_once(')')?.1;
    let fields: Vec<&str> = after.split_whitespace().collect();
    let ticks: u64 = fields
        .get(11..15)?
        .iter()
        .map(|field| field.parse::<u64>().ok())
        .sum::<Option<u64>>()?;
    Some(ticks * 1_000_000_000 / USER_HZ)
}

/// Reads a stream to its end, writing every line whole to the run log,
/// handing it whole to `sink`, and keeping the last [`TAIL_LINES`] (each
/// cut to 1 KiB).
/// The lines a run refuses, and the first refused line seen, with why.
struct Watch<'a> {
    refused: &'a [(String, String)],
    /// How many times in a row a line may come, and why more fails.
    repeated: Option<&'a (usize, String)>,
    seen: OnceLock<String>,
}

fn tail(
    stream: Option<impl Read>,
    which: Stream,
    sink: Option<&LineFn>,
    watch: &Watch<'_>,
    log: &RunLog,
) -> Vec<String> {
    let Some(stream) = stream else {
        return Vec::new();
    };
    let mut reader = BufReader::new(stream);
    let mut lines = VecDeque::with_capacity(TAIL_LINES);
    let mut buffer = Vec::new();
    // The line before and how many times in a row it came.
    let (mut last, mut times) = (Vec::new(), 0usize);
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
        if buffer == last {
            times += 1;
        } else {
            last.clone_from(&buffer);
            times = 1;
        }
        let mut line = String::from_utf8_lossy(&buffer).into_owned();
        if let Some((limit, why)) = watch.repeated
            && times == *limit
        {
            let _ = watch
                .seen
                .set(format!("{why} (it printed {limit} times in a row: {line})"));
        }
        log.line(
            match which {
                Stream::Stdout => "out",
                Stream::Stderr => "err",
            },
            &line,
        );
        // The sink has the line whole, as the log does: what it reads may
        // be one long line (a tool's JSON). Only the kept tail is cut.
        if let Some(sink) = sink {
            sink(which, &line);
        }
        if let Some((_, why)) = watch
            .refused
            .iter()
            .find(|(text, _)| line.contains(text.as_str()))
        {
            let _ = watch.seen.set(format!("{why} (it printed: {line})"));
        }
        if line.len() > LINE_BYTES {
            let mut end = LINE_BYTES;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            line.truncate(end);
        }
        if lines.len() == TAIL_LINES {
            lines.pop_front();
        }
        lines.push_back(line);
    }
    lines.into()
}
