//! What a step's failure carries.

use daedalus_cpu_author::FixedError;
use std::fmt;

/// A step's failure. Its reason is short and fixed in form,
/// `<step>: exited 3` or `<step>: cancelled`, since Daedalus keeps the first
/// 120 bytes of [`FixedError::reason`] in its fault record, and says when it cut one. What the tool
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
        let reason = format!("{step}: {what}");
        remember(&reason);
        Self(Box::new(Inner {
            step: step.to_string(),
            kind,
            stderr_tail,
            log: None,
            reason,
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

/// Every reason a step error of this process was made with, whole. The
/// graph's runtime keeps a fixed length of a reason, and a step that refuses
/// before it runs anything has no run log to hold the rest; the whole text
/// is kept here so the build can still say it (see [`whole_reason`]).
static REASONS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn remember(reason: &str) {
    if let Ok(mut reasons) = REASONS.lock() {
        // The same refusal made again (a retry) is kept once.
        if !reasons.iter().any(|known| known == reason) {
            reasons.push(reason.to_string());
        }
    }
}

/// The whole reason of a step error of this process whose text begins with
/// `cut`, a reason the runtime shortened. None when no error of this process
/// began so, or when more than one did and they differ after it: a guess
/// between two is worse than the cut text.
pub fn whole_reason(cut: &str) -> Option<String> {
    let reasons = REASONS.lock().ok()?;
    let mut found = reasons.iter().filter(|reason| reason.starts_with(cut));
    let first = found.next()?;
    found.next().is_none().then(|| first.clone())
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

#[cfg(test)]
mod whole_reason_tests {
    use super::{StepError, whole_reason};

    /// A reason is found again by its beginning, whole; two that begin the
    /// same and differ later give none; one never made gives none.
    #[test]
    fn a_cut_reason_is_found_whole_by_its_beginning() {
        let long = "the environment would be saved over the area 'u-boot' at 0x8000; turn the raw offsets off";
        let _ = StepError::body("test.whole-reason.a", long);
        assert_eq!(
            whole_reason("test.whole-reason.a: the environment would be saved over").as_deref(),
            Some(format!("test.whole-reason.a: {long}").as_str())
        );
        // The same error again is one reason, not two.
        let _ = StepError::body("test.whole-reason.a", long);
        assert!(whole_reason("test.whole-reason.a: the environment").is_some());
        let _ = StepError::body("test.whole-reason.b", "same start, one end");
        let _ = StepError::body("test.whole-reason.b", "same start, another end");
        assert_eq!(whole_reason("test.whole-reason.b: same start"), None);
        assert_eq!(whole_reason("test.whole-reason.c: never made"), None);
    }
}
