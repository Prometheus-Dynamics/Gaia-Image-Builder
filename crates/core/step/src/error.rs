//! What a step's failure carries.

use daedalus_cpu_author::FixedError;
use std::fmt;

/// A step's failure. Its reason is short and fixed in form,
/// `<step>: exited 3` or `<step>: cancelled`, since Daedalus keeps the first
/// 64 bytes of [`FixedError::reason`] in its fault record. What the tool
/// printed is in its run log ([`StepError::log`]) and, last lines only, in
/// [`StepError::stderr_tail`]; [`Display`] gives all three.
///
/// [`Display`]: fmt::Display
#[derive(Debug)]
pub struct StepError(Box<Inner>);

#[derive(Debug)]
struct Inner {
    step: String,
    kind: ErrorKind,
    stderr_tail: Vec<String>,
    log: Option<std::path::PathBuf>,
    reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// The tool exited with this status.
    Exit(i32),
    /// The tool was killed by this signal.
    Signal(i32),
    /// The run was cancelled and the tool's process group killed.
    Cancelled,
    /// The tool could not be started.
    Spawn(String),
    /// The content store failed.
    Store(String),
    /// The step's own body failed.
    Body(String),
}

/// Fault codes, for [`FixedError::code`]. A tool's own exit status is its
/// code, so these sit above 255.
pub mod code {
    pub const SIGNAL_BASE: u32 = 256;
    pub const CANCELLED: u32 = 1000;
    pub const SPAWN: u32 = 1001;
    pub const STORE: u32 = 1002;
    pub const BODY: u32 = 1003;
}

impl StepError {
    pub fn new(step: &str, kind: ErrorKind, stderr_tail: Vec<String>) -> Self {
        let what = match &kind {
            ErrorKind::Exit(status) => format!("exited {status}"),
            ErrorKind::Signal(signal) => format!("killed by signal {signal}"),
            ErrorKind::Cancelled => "cancelled".to_string(),
            ErrorKind::Spawn(why) => format!("could not start: {why}"),
            ErrorKind::Store(why) => format!("store: {why}"),
            ErrorKind::Body(why) => why.clone(),
        };
        Self(Box::new(Inner {
            step: step.to_string(),
            kind,
            stderr_tail,
            log: None,
            reason: format!("{step}: {what}"),
        }))
    }

    /// The step's name.
    pub fn step(&self) -> &str {
        &self.0.step
    }

    pub fn kind(&self) -> &ErrorKind {
        &self.0.kind
    }

    /// The tool's last stderr lines, oldest first, when a tool failed.
    pub fn stderr_tail(&self) -> &[String] {
        &self.0.stderr_tail
    }

    /// The run log of the tool that failed, with everything it printed.
    pub fn log(&self) -> Option<&std::path::Path> {
        self.0.log.as_deref()
    }

    /// The same error, pointing at the run log that holds the tool's output.
    pub fn with_log(mut self, log: std::path::PathBuf) -> Self {
        self.0.log = Some(log);
        self
    }

    /// A failure of the step's own code, not of a tool.
    pub fn body(step: &str, why: impl fmt::Display) -> Self {
        Self::new(step, ErrorKind::Body(why.to_string()), Vec::new())
    }

    /// For `map_err` on a store call.
    pub fn store(step: &str) -> impl Fn(gaia_store::Error) -> Self + '_ {
        move |error| Self::new(step, ErrorKind::Store(error.to_string()), Vec::new())
    }
}

impl FixedError for StepError {
    fn code(&self) -> u32 {
        match &self.0.kind {
            ErrorKind::Exit(status) => u32::try_from(*status).unwrap_or(1),
            ErrorKind::Signal(signal) => code::SIGNAL_BASE + signal.unsigned_abs(),
            ErrorKind::Cancelled => code::CANCELLED,
            ErrorKind::Spawn(_) => code::SPAWN,
            ErrorKind::Store(_) => code::STORE,
            ErrorKind::Body(_) => code::BODY,
        }
    }

    fn reason(&self) -> &str {
        &self.0.reason
    }
}

impl fmt::Display for StepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.reason)?;
        if let Some(log) = &self.0.log {
            write!(f, "\nlog: {}", log.display())?;
        }
        if !self.0.stderr_tail.is_empty() {
            f.write_str("\nstderr, last lines:")?;
            for line in &self.0.stderr_tail {
                write!(f, "\n  {line}")?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for StepError {}
