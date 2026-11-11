//! What a step's failure carries.

use daedalus_cpu_author::FixedError;
use std::fmt;

/// A step's failure. It names the step and holds what the tool printed last
/// on stderr. Daedalus keeps the first 64 bytes of [`FixedError::reason`] in
/// its fault record, so the reason starts with the step's name; [`Display`]
/// gives everything.
///
/// [`Display`]: fmt::Display
#[derive(Debug)]
pub struct StepError {
    pub step: String,
    pub kind: ErrorKind,
    /// The tool's last stderr lines, oldest first, when a tool failed.
    pub stderr_tail: Vec<String>,
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
        let reason = match stderr_tail.last() {
            Some(line) => format!("{step}: {what}: {line}"),
            None => format!("{step}: {what}"),
        };
        Self {
            step: step.to_string(),
            kind,
            stderr_tail,
            reason,
        }
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
        match &self.kind {
            ErrorKind::Exit(status) => u32::try_from(*status).unwrap_or(1),
            ErrorKind::Signal(signal) => code::SIGNAL_BASE + signal.unsigned_abs(),
            ErrorKind::Cancelled => code::CANCELLED,
            ErrorKind::Spawn(_) => code::SPAWN,
            ErrorKind::Store(_) => code::STORE,
            ErrorKind::Body(_) => code::BODY,
        }
    }

    fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for StepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)?;
        if self.stderr_tail.len() > 1 {
            f.write_str("\nstderr, last lines:")?;
            for line in &self.stderr_tail {
                write!(f, "\n  {line}")?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for StepError {}
