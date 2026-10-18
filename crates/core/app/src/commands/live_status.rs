//! The live status of a `gaia run`, kept in its build dir so another process
//! (`gaia status`, the TUI monitor) can follow the run.
//!
//! While the run executes, `.gaia-run.status.json` is rewritten through a
//! temp file and a rename: at most once a second for log and progress
//! updates, and at once for every operation start and finish. When the run
//! ends, its final snapshot and outcome go to `.gaia-run.last.json` and the
//! live file is removed.
//!
//! The snapshot carries everything the TUI run dashboard shows: every planned
//! operation with its state, timing, expected duration and reason, the
//! Buildroot packages, the step times and the variant. Each field added after
//! the first version is `#[serde(default)]`, so older files still read and
//! older readers ignore what they do not know.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gaia_exec::{ExecutionEvent, ExecutionOutcome};
use serde::{Deserialize, Serialize};

mod cache;
mod tracker;
pub(crate) use cache::SnapshotCache;
pub(crate) use tracker::{Change, RunTracker, live_variant, plan_operations, run_selection_label};

/// The live snapshot of a run in progress.
pub(crate) const STATUS_FILE: &str = ".gaia-run.status.json";
/// The final snapshot of the last run, with its outcome.
pub(crate) const LAST_FILE: &str = ".gaia-run.last.json";

const WRITE_INTERVAL: Duration = Duration::from_secs(1);

/// What a live `gaia run` looked like at its last write.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Every planned operation, in plan order, with its state.
    #[serde(default)]
    pub operations: Vec<LiveOperation>,
    /// The variant being built, for a build with variants.
    #[serde(default)]
    pub variant: Option<LiveVariant>,
    /// The errors that failed operations so far, oldest first, capped.
    #[serde(default)]
    pub errors: Vec<RunError>,
    /// What the run builds for: `target · profile`, when the build has them.
    #[serde(default)]
    pub selection: Option<String>,
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BuildProgressStatus {
    pub done: usize,
    pub total: usize,
    pub active: Vec<String>,
    pub eta_secs: Option<u64>,
    /// When each active package was first seen (unix milliseconds).
    #[serde(default)]
    pub active_since: Vec<PackageStart>,
    /// Packages that left the active list, oldest first, capped.
    #[serde(default)]
    pub finished: Vec<PackageTiming>,
}

/// A package the Buildroot make is building, and when it started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PackageStart {
    pub name: String,
    pub started_ms: u64,
}

/// A package the Buildroot make finished, and how long it was active.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PackageTiming {
    pub name: String,
    pub ms: u64,
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

/// The state of one planned operation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OpState {
    #[default]
    Pending,
    Running,
    Done,
    Reused,
    Failed,
    Cancelled,
    Skipped,
    /// A state written by a newer Gaia.
    #[serde(other)]
    Unknown,
}

/// One planned operation of a run, with what the run has done to it so far.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct LiveOperation {
    pub id: String,
    /// The part of the id before the colon: `image`, `artifact`, ...
    pub domain: String,
    pub state: OpState,
    /// The plan reuses the operation: it will not execute.
    pub planned_reuse: bool,
    /// Why the operation executes, when it does (a rebuild reason code).
    pub reason_code: Option<String>,
    pub reason: Option<String>,
    /// Recorded duration of the last run of this operation, when known.
    pub expected_ms: Option<u64>,
    /// Unix milliseconds.
    pub started_ms: Option<u64>,
    /// Unix milliseconds.
    pub ended_ms: Option<u64>,
    pub last_log: Option<String>,
    /// Inner progress of a Buildroot `make`, when the operation reports it.
    pub build: Option<BuildProgressStatus>,
    /// Step times the operation reported, in order, capped.
    pub steps: Vec<LiveStep>,
}

/// A step inside an operation and its wall time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LiveStep {
    pub step: String,
    pub ms: u64,
}

/// The variant a run builds, and its place among the declared variants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LiveVariant {
    pub id: String,
    /// 1-based position among the declared variants.
    pub index: usize,
    pub count: usize,
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

/// Identifies the run in its status files, and what it will do.
pub(crate) struct LiveRunInfo {
    pub build_name: String,
    pub display_name: String,
    pub ops_total: usize,
    /// The planned operations, in plan order (see [`plan_operations`]).
    pub operations: Vec<LiveOperation>,
    pub variant: Option<LiveVariant>,
    /// What the run builds for (see [`run_selection_label`]).
    pub selection: Option<String>,
}

/// Writes a run's live status. Feed it every event with [`record`](Self::record)
/// and call [`flush`](Self::flush) regularly; nothing is written until then.
pub(crate) struct LiveRecorder {
    status_path: PathBuf,
    last_path: PathBuf,
    tracker: RunTracker,
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
            tracker: RunTracker::new(info),
            dirty: true,
            urgent: false,
            last_write: None,
            written_paused: false,
        }
    }

    /// Records the state change an execution event causes.
    pub(crate) fn record(&mut self, event: &ExecutionEvent) {
        match self.tracker.record(event, unix_now_ms()) {
            Change::Lifecycle => self.mark_lifecycle(),
            Change::Output => self.dirty = true,
        }
    }

    fn mark_lifecycle(&mut self) {
        self.dirty = true;
        self.urgent = true;
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
        self.tracker.snapshot(paused, unix_now_ms())
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

/// The part of an operation id before its colon: `image` for `image:buildroot`.
pub(crate) fn domain_of(id: &str) -> String {
    id.split_once(':')
        .map_or(id, |(domain, _)| domain)
        .to_string()
}

pub(crate) fn unix_now() -> u64 {
    unix_now_ms() / 1000
}

/// Unix time in milliseconds.
pub(crate) fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
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
                ..BuildProgressStatus::default()
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
        ..LiveStatus::default()
    }
}

#[cfg(test)]
mod tests {
    use super::cache::FileCache;
    use super::*;
    use gaia_plan::OperationId;
    use gaia_testkit::TempDir;

    fn id(value: &str) -> OperationId {
        OperationId::new(value)
    }

    fn read_status(build_dir: &Path) -> Option<LiveStatus> {
        read_json(&build_dir.join(STATUS_FILE))
    }

    fn temp_dir(name: &str) -> TempDir {
        TempDir::new(&format!("gaia-live-status-{name}"))
    }

    fn info() -> LiveRunInfo {
        LiveRunInfo {
            build_name: "demo".into(),
            display_name: "Demo build".into(),
            ops_total: 4,
            operations: Vec::new(),
            variant: None,
            selection: None,
        }
    }

    fn recorder(dir: &Path) -> LiveRecorder {
        LiveRecorder::new(dir, info())
    }

    #[test]
    fn file_cache_reads_a_changed_file_again() {
        let dir = temp_dir("file-cache");
        let path = dir.path().join("values.json");
        let mut cache = FileCache::<u32>::default();
        assert_eq!(cache.read(&path), None);
        fs::write(&path, "1").expect("write");
        assert_eq!(cache.read(&path), Some(1));
        // Same content, same stamp: the cached value is kept.
        assert_eq!(cache.read(&path), Some(1));
        fs::write(&path, "1000").expect("rewrite");
        assert_eq!(cache.read(&path), Some(1000));
        fs::remove_file(&path).expect("remove");
        assert_eq!(cache.read(&path), None);
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
    fn snapshot_without_the_dashboard_fields_still_reads() {
        // A snapshot as the first version wrote it: no operations, variant or
        // errors, and no package timing inside the build progress.
        let mut value = serde_json::to_value(fixture_status()).expect("encode");
        let object = value.as_object_mut().expect("object");
        object.remove("operations");
        object.remove("variant");
        object.remove("errors");
        let build = &mut value["running"][0]["build"];
        build.as_object_mut().expect("build").remove("active_since");
        build.as_object_mut().expect("build").remove("finished");

        let status: LiveStatus = serde_json::from_value(value).expect("old snapshot");
        assert!(status.operations.is_empty());
        assert!(status.variant.is_none());
        assert!(status.errors.is_empty());
        assert_eq!(status.running[0].build.as_ref().map(|b| b.done), Some(120));
    }

    #[test]
    fn new_fields_round_trip_and_unknown_states_read() {
        let mut status = fixture_status();
        status.variant = Some(LiveVariant {
            id: "rel2026".into(),
            index: 2,
            count: 4,
        });
        status.selection = Some("rpi5 · release".into());
        status.operations = vec![LiveOperation {
            id: "artifact:cli".into(),
            domain: "artifact".into(),
            state: OpState::Done,
            expected_ms: Some(1_500),
            started_ms: Some(1_000),
            ended_ms: Some(2_500),
            ..LiveOperation::default()
        }];
        let json = serde_json::to_string(&status).expect("encode");
        let decoded: LiveStatus = serde_json::from_str(&json).expect("decode");
        assert_eq!(decoded, status);

        // A state a newer Gaia writes reads as Unknown rather than failing.
        let mut value = serde_json::to_value(&status).expect("encode");
        value["operations"][0]["state"] = serde_json::json!("quantum");
        let decoded: LiveStatus = serde_json::from_value(value).expect("decode");
        assert_eq!(decoded.operations[0].state, OpState::Unknown);
    }

    #[test]
    fn recorder_writes_on_start_and_throttles_log_updates() {
        let dir_scratch = temp_dir("throttle");
        let dir = dir_scratch.path().to_path_buf();
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
    }

    #[test]
    fn lifecycle_events_are_written_at_once() {
        let dir_scratch = temp_dir("urgent");
        let dir = dir_scratch.path().to_path_buf();
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
    }

    #[test]
    fn finish_leaves_last_snapshot_and_removes_live_file() {
        let dir_scratch = temp_dir("finish");
        let dir = dir_scratch.path().to_path_buf();
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
    }

    #[test]
    fn last_snapshot_without_errors_field_still_reads() {
        let dir_scratch = temp_dir("old-last");
        let dir = dir_scratch.path().to_path_buf();
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
    }

    #[test]
    fn build_progress_is_kept_on_the_running_operation() {
        let dir_scratch = temp_dir("progress");
        let dir = dir_scratch.path().to_path_buf();
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
    }
}
