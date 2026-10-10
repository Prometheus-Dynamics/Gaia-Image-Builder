//! The live status of a `gaia run`, kept in its build dir so another process
//! (`gaia status`, the TUI monitor) can follow the run.
//!
//! While the run executes, `.gaia-run.status.json` is rewritten through a
//! temp file and a rename: at most once a second for log and progress
//! updates, and at once for every operation start and finish. When the run
//! ends, its final snapshot and outcome go to `.gaia-run.last.json` and the
//! live file is removed.

use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gaia_exec::{ExecutionEvent, ExecutionOutcome};
use serde::{Deserialize, Serialize};

use super::progress::compact_log_line;

/// The live snapshot of a run in progress.
pub(crate) const STATUS_FILE: &str = ".gaia-run.status.json";
/// The final snapshot of the last run, with its outcome.
pub(crate) const LAST_FILE: &str = ".gaia-run.last.json";

const LOG_CAPACITY: usize = 200;
const FAILED_CAPACITY: usize = 20;
const WRITE_INTERVAL: Duration = Duration::from_secs(1);

/// What a live `gaia run` looked like at its last write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LiveStatus {
    pub pid: u32,
    pub build_name: String,
    pub display_name: String,
    /// Unix seconds when the run started.
    pub started_at: u64,
    /// Unix seconds of this snapshot.
    pub updated_at: u64,
    pub paused: bool,
    pub ops_total: usize,
    /// Operations in a final state: done, reused, failed, cancelled or skipped.
    pub ops_done: usize,
    pub running: Vec<RunningOperation>,
    pub finished: FinishedCounts,
    /// Ids of failed operations, oldest first, capped.
    pub failed: Vec<String>,
    /// The most recent output lines, oldest first.
    pub logs: Vec<LogLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RunningOperation {
    pub id: String,
    /// Unix seconds when the operation started.
    pub started_at: u64,
    pub last_log: Option<String>,
    /// Inner progress of a Buildroot `make`, when the operation reports it.
    pub build: Option<BuildProgressStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BuildProgressStatus {
    pub done: usize,
    pub total: usize,
    pub active: Vec<String>,
    pub eta_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FinishedCounts {
    pub done: usize,
    pub reused: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub skipped: usize,
}

impl FinishedCounts {
    pub(crate) fn total(&self) -> usize {
        self.done + self.reused + self.failed + self.cancelled + self.skipped
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LogLine {
    pub op: String,
    pub line: String,
}

/// The final snapshot of a run and how it ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LastRun {
    /// `completed`, `failed` or `cancelled`.
    pub outcome: String,
    /// Unix seconds when the run ended.
    pub ended_at: u64,
    pub status: LiveStatus,
    /// The errors that ended operations, in the order they were raised. Empty
    /// for a snapshot written before errors were kept.
    #[serde(default)]
    pub errors: Vec<RunError>,
}

/// One error that ended an operation, with its message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RunError {
    pub operation_id: String,
    pub code: String,
    pub message: String,
}

/// The errors of an execution outcome, for the last-run file.
pub(crate) fn run_errors(outcome: &ExecutionOutcome) -> Vec<RunError> {
    outcome
        .errors
        .iter()
        .map(|error| RunError {
            operation_id: error.operation_id.as_str().to_string(),
            code: error.code.to_string(),
            message: error.message.clone(),
        })
        .collect()
}

/// A `gaia run` found alive through its pid file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveRun {
    pub pid: u32,
    /// `None` until the run writes its first snapshot.
    pub status: Option<LiveStatus>,
    /// The process is stopped (SIGSTOP), as a paused run is; it cannot write
    /// its status while it is.
    pub stopped: bool,
}

impl LiveRun {
    pub(crate) fn paused(&self) -> bool {
        self.stopped || self.status.as_ref().is_some_and(|status| status.paused)
    }
}

/// Identifies the run in its status files.
pub(crate) struct LiveRunInfo {
    pub build_name: String,
    pub display_name: String,
    pub ops_total: usize,
}

/// Writes a run's live status. Feed it every event with [`record`](Self::record)
/// and call [`flush`](Self::flush) regularly; nothing is written until then.
pub(crate) struct LiveRecorder {
    status_path: PathBuf,
    last_path: PathBuf,
    pid: u32,
    info: LiveRunInfo,
    started_at: u64,
    running: Vec<RunningOperation>,
    finished: FinishedCounts,
    failed: Vec<String>,
    logs: VecDeque<LogLine>,
    /// Something changed since the last write.
    dirty: bool,
    /// An operation started or finished: write at once, regardless of the
    /// interval.
    urgent: bool,
    last_write: Option<Instant>,
    written_paused: bool,
}

impl LiveRecorder {
    pub(crate) fn new(build_dir: &Path, info: LiveRunInfo) -> Self {
        Self {
            status_path: build_dir.join(STATUS_FILE),
            last_path: build_dir.join(LAST_FILE),
            pid: std::process::id(),
            info,
            started_at: unix_now(),
            running: Vec::new(),
            finished: FinishedCounts::default(),
            failed: Vec::new(),
            logs: VecDeque::new(),
            dirty: true,
            urgent: false,
            last_write: None,
            written_paused: false,
        }
    }

    /// Records the state change an execution event causes.
    pub(crate) fn record(&mut self, event: &ExecutionEvent) {
        match event {
            ExecutionEvent::Started { operation_id } => {
                let id = operation_id.as_str();
                if !self.running.iter().any(|op| op.id == id) {
                    self.running.push(RunningOperation {
                        id: id.to_string(),
                        started_at: unix_now(),
                        last_log: None,
                        build: None,
                    });
                }
                self.mark_lifecycle();
            }
            ExecutionEvent::Log {
                operation_id,
                message,
            } => {
                let id = operation_id.as_str();
                if let Some(progress) = gaia_process::parse_build_progress(message) {
                    if let Some(op) = self.running.iter_mut().find(|op| op.id == id) {
                        op.build = Some(BuildProgressStatus {
                            done: progress.done,
                            total: progress.total,
                            active: progress.active,
                            eta_secs: progress.eta.map(|eta| eta.as_secs()),
                        });
                    }
                } else {
                    for line in message.lines().map(compact_log_line) {
                        if line.is_empty() {
                            continue;
                        }
                        if let Some(op) = self.running.iter_mut().find(|op| op.id == id) {
                            op.last_log = Some(line.clone());
                        }
                        if self.logs.len() == LOG_CAPACITY {
                            self.logs.pop_front();
                        }
                        self.logs.push_back(LogLine {
                            op: id.to_string(),
                            line,
                        });
                    }
                }
                self.dirty = true;
            }
            ExecutionEvent::Succeeded { operation_id } => {
                self.finished.done += 1;
                self.end(operation_id.as_str());
            }
            ExecutionEvent::Reused { operation_id } => {
                self.finished.reused += 1;
                self.end(operation_id.as_str());
            }
            ExecutionEvent::Cancelled { operation_id } => {
                self.finished.cancelled += 1;
                self.end(operation_id.as_str());
            }
            ExecutionEvent::Failed { operation_id, .. } => {
                self.finished.failed += 1;
                if self.failed.len() < FAILED_CAPACITY {
                    self.failed.push(operation_id.as_str().to_string());
                }
                self.end(operation_id.as_str());
            }
            ExecutionEvent::Skipped { operation_id, .. } => {
                self.finished.skipped += 1;
                self.end(operation_id.as_str());
            }
        }
    }

    fn mark_lifecycle(&mut self) {
        self.dirty = true;
        self.urgent = true;
    }

    fn end(&mut self, id: &str) {
        self.running.retain(|op| op.id != id);
        self.mark_lifecycle();
    }

    /// Writes the snapshot when something changed and the interval allows it
    /// (lifecycle changes are never held back).
    pub(crate) fn flush(&mut self, now: Instant) {
        let paused = gaia_process::is_paused();
        let changed = self.dirty || paused != self.written_paused;
        let due = self
            .last_write
            .is_none_or(|last| now.duration_since(last) >= WRITE_INTERVAL);
        if !(self.urgent || (changed && due)) {
            return;
        }
        let snapshot = self.snapshot(paused);
        if write_json_atomic(&self.status_path, &snapshot).is_ok() {
            self.dirty = false;
            self.urgent = false;
            self.last_write = Some(now);
            self.written_paused = paused;
        }
    }

    /// Ends the run: leaves its final snapshot, outcome and errors in the
    /// last-run file and removes the live status file.
    pub(crate) fn finish(self, outcome: &str, errors: Vec<RunError>) {
        let last = LastRun {
            outcome: outcome.to_string(),
            ended_at: unix_now(),
            status: self.snapshot(false),
            errors,
        };
        let _ = write_json_atomic(&self.last_path, &last);
        let _ = fs::remove_file(&self.status_path);
    }

    fn snapshot(&self, paused: bool) -> LiveStatus {
        LiveStatus {
            pid: self.pid,
            build_name: self.info.build_name.clone(),
            display_name: self.info.display_name.clone(),
            started_at: self.started_at,
            updated_at: unix_now(),
            paused,
            ops_total: self.info.ops_total,
            ops_done: self.finished.total(),
            running: self.running.clone(),
            finished: self.finished.clone(),
            failed: self.failed.clone(),
            logs: self.logs.iter().cloned().collect(),
        }
    }
}

/// The outcome word for the last-run file.
pub(crate) fn run_outcome_label(outcome: &ExecutionOutcome) -> &'static str {
    if outcome.cancelled {
        "cancelled"
    } else if outcome.errors.is_empty() {
        "completed"
    } else {
        "failed"
    }
}

/// The files of one `gaia run`, by absolute path: its pid file and its live
/// and last-run snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunFiles {
    pub pid: PathBuf,
    pub status: PathBuf,
    pub last: PathBuf,
}

impl RunFiles {
    pub(crate) fn in_build_dir(build_dir: &Path) -> Self {
        Self {
            pid: build_dir.join(super::interrupt::RUN_PID_FILE),
            status: build_dir.join(STATUS_FILE),
            last: build_dir.join(LAST_FILE),
        }
    }
}

/// The live run whose pid file is `files.pid`, when that names a live gaia
/// process.
#[cfg(unix)]
pub(crate) fn live_run_at(files: &RunFiles) -> Option<LiveRun> {
    let pid = super::interrupt::running_gaia(&files.pid)?;
    let pid = u32::try_from(pid).ok()?;
    Some(live_run_for_pid(pid, &files.status))
}

#[cfg(not(unix))]
pub(crate) fn live_run_at(_files: &RunFiles) -> Option<LiveRun> {
    None
}

/// The state of the live gaia run `pid`, read with its status snapshot at
/// `status`. The caller has checked that the pid is a live gaia process.
pub(crate) fn live_run_for_pid(pid: u32, status: &Path) -> LiveRun {
    LiveRun {
        pid,
        status: read_json::<LiveStatus>(status).filter(|status| status.pid == pid),
        stopped: process_stopped(pid),
    }
}

/// The live run of `build_dir`, when its pid file names a live gaia process.
pub(crate) fn live_run(build_dir: &Path) -> Option<LiveRun> {
    live_run_at(&RunFiles::in_build_dir(build_dir))
}

/// The final snapshot of the last run of `build_dir`.
pub(crate) fn read_last(build_dir: &Path) -> Option<LastRun> {
    read_last_at(&build_dir.join(LAST_FILE))
}

/// The final snapshot at `path`.
pub(crate) fn read_last_at(path: &Path) -> Option<LastRun> {
    read_json(path)
}

pub(crate) fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// Writes `value` as JSON through a temp file, so readers never see a
/// partial file.
pub(crate) fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path)
}

/// Whether the process is stopped (state `T`), as a paused run is. Reads
/// `/proc`, so it is only ever true on Linux.
fn process_stopped(pid: u32) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // The command name is in parentheses and may contain spaces; the state
    // follows the last closing parenthesis.
    stat.rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .is_some_and(|state| matches!(state, "T" | "t"))
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) fn fixture_status() -> LiveStatus {
    LiveStatus {
        pid: 4242,
        build_name: "demo".into(),
        display_name: "Demo build".into(),
        started_at: 1_000,
        updated_at: 1_100,
        paused: false,
        ops_total: 10,
        ops_done: 3,
        running: vec![RunningOperation {
            id: "image:buildroot".into(),
            started_at: 1_050,
            last_log: Some("building linux".into()),
            build: Some(BuildProgressStatus {
                done: 120,
                total: 145,
                active: vec!["linux".into(), "mesa3d".into()],
                eta_secs: Some(600),
            }),
        }],
        finished: FinishedCounts {
            done: 2,
            reused: 1,
            ..FinishedCounts::default()
        },
        failed: Vec::new(),
        logs: vec![LogLine {
            op: "artifact:cli".into(),
            line: "cargo build finished".into(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_plan::OperationId;

    fn id(value: &str) -> OperationId {
        OperationId::new(value)
    }

    fn read_status(build_dir: &Path) -> Option<LiveStatus> {
        read_json(&build_dir.join(STATUS_FILE))
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("gaia-live-status-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn recorder(dir: &Path) -> LiveRecorder {
        LiveRecorder::new(
            dir,
            LiveRunInfo {
                build_name: "demo".into(),
                display_name: "Demo build".into(),
                ops_total: 4,
            },
        )
    }

    #[test]
    fn live_status_round_trips_through_json() {
        let status = fixture_status();
        let json = serde_json::to_string(&status).expect("encode");
        let decoded: LiveStatus = serde_json::from_str(&json).expect("decode");
        assert_eq!(decoded, status);

        let last = LastRun {
            outcome: "cancelled".into(),
            ended_at: 1_200,
            status,
            errors: Vec::new(),
        };
        let json = serde_json::to_string(&last).expect("encode");
        let decoded: LastRun = serde_json::from_str(&json).expect("decode");
        assert_eq!(decoded, last);
    }

    #[test]
    fn recorder_writes_on_start_and_throttles_log_updates() {
        let dir = temp_dir("throttle");
        let mut live = recorder(&dir);
        let start = Instant::now();
        live.record(&ExecutionEvent::Started {
            operation_id: id("artifact:cli"),
        });
        live.flush(start);
        let status_path = dir.join(STATUS_FILE);
        let written = read_status(&dir).expect("first snapshot");
        assert_eq!(written.running.len(), 1);
        assert!(written.logs.is_empty());

        // A log line inside the interval is held back...
        live.record(&ExecutionEvent::Log {
            operation_id: id("artifact:cli"),
            message: "compiling".into(),
        });
        live.flush(start + Duration::from_millis(400));
        assert!(read_status(&dir).expect("snapshot").logs.is_empty());

        // ...and written once a second has passed.
        live.flush(start + Duration::from_millis(1_200));
        let logs = read_status(&dir).expect("snapshot").logs;
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].line, "compiling");

        // No temp file is left behind by the atomic rename.
        assert!(status_path.exists());
        assert!(!dir.join(format!("{STATUS_FILE}.tmp")).exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn lifecycle_events_are_written_at_once() {
        let dir = temp_dir("urgent");
        let mut live = recorder(&dir);
        let start = Instant::now();
        live.flush(start);
        live.record(&ExecutionEvent::Started {
            operation_id: id("a"),
        });
        live.flush(start + Duration::from_millis(10));
        assert_eq!(read_status(&dir).expect("snapshot").running.len(), 1);

        live.record(&ExecutionEvent::Succeeded {
            operation_id: id("a"),
        });
        live.flush(start + Duration::from_millis(20));
        let status = read_status(&dir).expect("snapshot");
        assert!(status.running.is_empty());
        assert_eq!(status.finished.done, 1);
        assert_eq!(status.ops_done, 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn finish_leaves_last_snapshot_and_removes_live_file() {
        let dir = temp_dir("finish");
        let mut live = recorder(&dir);
        live.record(&ExecutionEvent::Started {
            operation_id: id("a"),
        });
        live.record(&ExecutionEvent::Failed {
            operation_id: id("a"),
            message: "boom".into(),
        });
        live.flush(Instant::now());
        assert!(dir.join(STATUS_FILE).exists());

        let errors = vec![RunError {
            operation_id: "a".into(),
            code: "backend_command_failed".into(),
            message: "boom".into(),
        }];
        live.finish("failed", errors.clone());
        assert!(!dir.join(STATUS_FILE).exists());
        let last = read_last(&dir).expect("last snapshot");
        assert_eq!(last.outcome, "failed");
        assert_eq!(last.status.finished.failed, 1);
        assert_eq!(last.status.failed, vec!["a".to_string()]);
        assert_eq!(last.errors, errors);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn last_snapshot_without_errors_field_still_reads() {
        let dir = temp_dir("old-last");
        let mut value = serde_json::to_value(LastRun {
            outcome: "failed".into(),
            ended_at: 1_200,
            status: fixture_status(),
            errors: Vec::new(),
        })
        .expect("encode");
        value.as_object_mut().expect("object").remove("errors");
        fs::write(dir.join(LAST_FILE), value.to_string()).expect("write");
        let last = read_last(&dir).expect("old snapshot");
        assert!(last.errors.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn build_progress_is_kept_on_the_running_operation() {
        let dir = temp_dir("progress");
        let mut live = recorder(&dir);
        live.record(&ExecutionEvent::Started {
            operation_id: id("image"),
        });
        live.record(&ExecutionEvent::Log {
            operation_id: id("image"),
            message: "build progress: 12/40 eta=300 active=linux,mesa3d".into(),
        });
        live.flush(Instant::now());
        let status = read_status(&dir).expect("snapshot");
        assert!(status.logs.is_empty(), "progress lines are not log lines");
        let build = status.running[0].build.clone().expect("build progress");
        assert_eq!(
            (build.done, build.total, build.eta_secs),
            (12, 40, Some(300))
        );
        assert_eq!(build.active, vec!["linux", "mesa3d"]);
        let _ = fs::remove_dir_all(dir);
    }
}
