//! Run logs: every tool run's whole output, in a file of its own.
//!
//! Logs live in a `logs` directory next to the store's root, one directory
//! per step name, one file per tool run, named so they sort by start time:
//!
//! ```text
//! <store parent>/logs/<step>/<start ns>-<key, 16 hex>-<pid>-<n>.log
//! ```
//!
//! A log is not a store entry and is not in the step's scratch, so a failed
//! step keeps it. Nothing deletes logs automatically.
//!
//! A log reads:
//!
//! ```text
//! # step test.tool
//! # key sha256:...
//! # tool /bin/sh -c ...
//! # start 1762800000000000000
//! out| a line the tool printed on stdout
//! err| a line on stderr
//! # end 1762800000150000000 exit 3
//! ```

use gaia_store::{Digest, ObjectRef, StepKey, Store};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The run log directory of `store`: `logs` next to its root.
pub fn logs_dir(store: &Store) -> PathBuf {
    store.root().with_file_name("logs")
}

/// Nanoseconds since the Unix epoch.
pub fn now_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default()
}

#[derive(Debug)]
pub(crate) struct RunLog {
    pub(crate) path: PathBuf,
    file: Mutex<File>,
}

impl RunLog {
    pub(crate) fn create(
        logs: &Path,
        step: &str,
        key: &StepKey,
        command: &str,
    ) -> io::Result<Self> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = logs.join(file_name(step));
        fs::create_dir_all(&dir)?;
        let start = now_ns();
        let path = dir.join(format!(
            "{start}-{}-{}-{}.log",
            &key.0.hex()[..16],
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = File::create(&path)?;
        writeln!(
            file,
            "# step {step}\n# key {}\n# tool {command}\n# start {start}",
            key.0
        )?;
        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }

    pub(crate) fn line(&self, prefix: &str, text: &str) {
        if let Ok(mut file) = self.file.lock() {
            let _ = writeln!(file, "{prefix}| {text}");
        }
    }

    pub(crate) fn end(&self, status: &str) {
        if let Ok(mut file) = self.file.lock() {
            let _ = writeln!(file, "# end {} {status}", now_ns());
            let _ = file.flush();
        }
    }
}

/// A step name as a directory name: anything but letters, digits, `.`, `-`
/// and `_` becomes `_`.
pub(crate) fn file_name(step: &str) -> String {
    step.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The step record's file in a run log directory.
const STEPS: &str = "steps.log";

/// What a step adds to its record beyond its outputs: how long its tools
/// ran, the job count it was given, whether it failed, and the details its
/// body reported.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// User and system CPU time of the processes its tools ran, and of the
    /// children those processes waited for, in nanoseconds.
    #[serde(default)]
    pub cpu_ns: u64,
    /// The job count the step was given, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jobs: Option<u32>,
    /// Whether the body failed.
    #[serde(default, skip_serializing_if = "is_false")]
    pub failed: bool,
    /// What the body reported with [`crate::Work::detail`], by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, serde_json::Value>,
}

/// For `skip_serializing_if`: a flag that is off is left out.
fn is_false(flag: &bool) -> bool {
    !*flag
}

/// One look-up-or-make, as the step record has it.
///
/// Every [`crate::Step::look_up_or_make`] that ends with outputs, and every
/// one whose body fails, appends one line to `steps.log` in the run log
/// directory:
///
/// ```text
/// <start ns> <end ns> <pid> hit|made <step> <key> <output>=<object> ...<TAB><report JSON>
/// ```
///
/// `hit` means the entry was found and the body did not run. A step that
/// was given a RAM work directory (see [`crate::Step::ram_work`]) also has
/// `work=<bytes>` after its outputs, the size its work directory had when
/// the body ended; the next run of that step expects about that much. The
/// report is after a tab, as one JSON object on the line; a line without one
/// (written before reports) has an empty report. Like the run logs, this is
/// for people and tests to read, and for that expectation.
#[derive(Debug, Clone, PartialEq)]
pub struct StepRecord {
    pub start_ns: u128,
    pub end_ns: u128,
    pub pid: u32,
    /// Whether the body ran.
    pub made: bool,
    pub step: String,
    pub key: StepKey,
    pub outputs: BTreeMap<String, ObjectRef>,
    /// The size of the step's work directory when its body ended, if it was
    /// measured.
    pub work_bytes: Option<u64>,
    pub report: Report,
}

impl StepRecord {
    fn line(&self) -> String {
        let mut line = format!(
            "{} {} {} {} {} {}",
            self.start_ns,
            self.end_ns,
            self.pid,
            if self.made { "made" } else { "hit" },
            file_name(&self.step),
            self.key.0
        );
        for (name, object) in &self.outputs {
            line.push_str(&format!(" {name}={object}"));
        }
        if let Some(bytes) = self.work_bytes {
            line.push_str(&format!(" work={bytes}"));
        }
        let report = serde_json::to_string(&self.report).unwrap_or_else(|_| "{}".into());
        line.push('\t');
        line.push_str(&report);
        line.push('\n');
        line
    }

    fn parse(line: &str) -> Option<Self> {
        let (line, report) = match line.split_once('\t') {
            Some((line, report)) => (line, serde_json::from_str(report).ok()?),
            None => (line, Report::default()),
        };
        let mut words = line.split_whitespace();
        let start_ns = words.next()?.parse().ok()?;
        let end_ns = words.next()?.parse().ok()?;
        let pid = words.next()?.parse().ok()?;
        let made = match words.next()? {
            "made" => true,
            "hit" => false,
            _ => return None,
        };
        let step = words.next()?.to_string();
        let key = StepKey(Digest::parse(words.next()?)?);
        let mut outputs = BTreeMap::new();
        let mut work_bytes = None;
        for word in words {
            if let Some(bytes) = word.strip_prefix("work=") {
                work_bytes = Some(bytes.parse().ok()?);
                continue;
            }
            let (name, object) = word.split_once('=')?;
            outputs.insert(name.to_string(), object.parse().ok()?);
        }
        Some(Self {
            start_ns,
            end_ns,
            pid,
            made,
            step,
            key,
            outputs,
            work_bytes,
            report,
        })
    }
}

/// Appends `record` to the step record in `logs`, as one write.
pub(crate) fn record_step(logs: &Path, record: &StepRecord) -> io::Result<()> {
    fs::create_dir_all(logs)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs.join(STEPS))?;
    file.write_all(record.line().as_bytes())
}

/// The step records written at or after byte `offset` of the step record in
/// `logs`, and the offset to read from next time. Only whole lines are read,
/// so a line still being written is left for the next call.
pub fn step_records_from(logs: &Path, offset: u64) -> (Vec<StepRecord>, u64) {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = File::open(logs.join(STEPS)) else {
        return (Vec::new(), offset);
    };
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(offset)).is_err() || file.read_to_end(&mut bytes).is_err() {
        return (Vec::new(), offset);
    }
    let whole = bytes
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |at| at + 1);
    let records = String::from_utf8_lossy(&bytes[..whole])
        .lines()
        .filter_map(StepRecord::parse)
        .collect();
    (records, offset + whole as u64)
}

/// Every step record in `logs`, in the order written. Lines that don't parse
/// are skipped.
pub fn step_records(logs: &Path) -> Vec<StepRecord> {
    fs::read_to_string(logs.join(STEPS))
        .unwrap_or_default()
        .lines()
        .filter_map(StepRecord::parse)
        .collect()
}
