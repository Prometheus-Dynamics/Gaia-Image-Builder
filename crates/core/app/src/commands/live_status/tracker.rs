//! The run state that the live status snapshot and the TUI dashboard are
//! built from. A [`RunTracker`] is fed every execution event as it happens
//! and answers with a [`LiveStatus`] snapshot at any time, so `gaia run`
//! (through [`LiveRecorder`](super::LiveRecorder)) and the in-process TUI show
//! the same run.

use std::collections::{BTreeMap, VecDeque};

use gaia_exec::ExecutionEvent;
use gaia_plan::{OperationReuse, PlannedOperation};
use gaia_process::{BuildProgress, parse_build_progress, parse_step_time};
use gaia_spec::{DEFAULT_VARIANT_ID, ResolvedBuildSpec};

use super::super::progress::compact_log_line;
use super::*;

/// Output lines kept in a snapshot, newest last.
const LOG_CAPACITY: usize = 200;
/// Failed operation ids kept in a snapshot.
const FAILED_CAPACITY: usize = 20;
/// Errors kept in a snapshot.
const ERROR_CAPACITY: usize = 50;
/// Step times kept per operation.
const STEP_CAPACITY: usize = 100;
/// Finished Buildroot packages kept per operation.
const PACKAGE_CAPACITY: usize = 200;

/// What an event changed: a lifecycle change is written at once, output is
/// written on the usual interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    Lifecycle,
    Output,
}

/// The state of a run, built from its execution events.
pub(crate) struct RunTracker {
    build_name: String,
    display_name: String,
    ops_total: usize,
    pid: u32,
    /// Unix milliseconds when the run started.
    started_ms: u64,
    variant: Option<LiveVariant>,
    selection: Option<String>,
    ops: Vec<LiveOperation>,
    index: BTreeMap<String, usize>,
    counts: FinishedCounts,
    failed: Vec<String>,
    errors: Vec<RunError>,
    logs: VecDeque<LogLine>,
}

impl RunTracker {
    pub(crate) fn new(info: LiveRunInfo) -> Self {
        let index = info
            .operations
            .iter()
            .enumerate()
            .map(|(position, operation)| (operation.id.clone(), position))
            .collect();
        Self {
            build_name: info.build_name,
            display_name: info.display_name,
            ops_total: info.ops_total,
            pid: std::process::id(),
            started_ms: unix_now_ms(),
            variant: info.variant,
            selection: info.selection,
            ops: info.operations,
            index,
            counts: FinishedCounts::default(),
            failed: Vec::new(),
            errors: Vec::new(),
            logs: VecDeque::new(),
        }
    }

    /// Records what an event changed, at `now_ms` (unix milliseconds).
    pub(crate) fn record(&mut self, event: &ExecutionEvent, now_ms: u64) -> Change {
        match event {
            ExecutionEvent::Started { operation_id } => {
                let op = self.op_mut(operation_id.as_str());
                if op.state != OpState::Running {
                    op.state = OpState::Running;
                    op.started_ms = Some(now_ms);
                    op.ended_ms = None;
                    op.last_log = None;
                }
                Change::Lifecycle
            }
            ExecutionEvent::Log {
                operation_id,
                message,
            } => {
                self.record_output(operation_id.as_str(), message, now_ms);
                Change::Output
            }
            ExecutionEvent::Succeeded { operation_id } => {
                self.counts.done += 1;
                self.end(operation_id.as_str(), OpState::Done, now_ms);
                Change::Lifecycle
            }
            ExecutionEvent::Reused { operation_id } => {
                self.counts.reused += 1;
                self.end(operation_id.as_str(), OpState::Reused, now_ms);
                Change::Lifecycle
            }
            ExecutionEvent::Cancelled { operation_id } => {
                self.counts.cancelled += 1;
                self.end(operation_id.as_str(), OpState::Cancelled, now_ms);
                Change::Lifecycle
            }
            ExecutionEvent::Failed {
                operation_id,
                message,
            } => {
                self.counts.failed += 1;
                let id = operation_id.as_str().to_string();
                push_capped(&mut self.failed, id.clone(), FAILED_CAPACITY);
                push_capped(
                    &mut self.errors,
                    RunError {
                        operation_id: id.clone(),
                        code: String::new(),
                        message: message.clone(),
                    },
                    ERROR_CAPACITY,
                );
                self.end(&id, OpState::Failed, now_ms);
                Change::Lifecycle
            }
            ExecutionEvent::Skipped {
                operation_id,
                reason,
            } => {
                self.counts.skipped += 1;
                let id = operation_id.as_str();
                self.op_mut(id).reason = Some(reason.clone());
                self.end(id, OpState::Skipped, now_ms);
                Change::Lifecycle
            }
        }
    }

    /// The state of the run now, as the live status file holds it.
    pub(crate) fn snapshot(&self, paused: bool, now_ms: u64) -> LiveStatus {
        LiveStatus {
            pid: self.pid,
            build_name: self.build_name.clone(),
            display_name: self.display_name.clone(),
            started_at: self.started_ms / 1000,
            updated_at: now_ms / 1000,
            paused,
            ops_total: self.ops_total,
            ops_done: self.counts.total(),
            running: self
                .ops
                .iter()
                .filter(|op| op.state == OpState::Running)
                .map(|op| RunningOperation {
                    id: op.id.clone(),
                    started_at: op.started_ms.unwrap_or(now_ms) / 1000,
                    last_log: op.last_log.clone(),
                    build: op.build.clone(),
                })
                .collect(),
            finished: self.counts.clone(),
            failed: self.failed.clone(),
            logs: self.logs.iter().cloned().collect(),
            operations: self.ops.clone(),
            variant: self.variant.clone(),
            selection: self.selection.clone(),
            errors: self.errors.clone(),
        }
    }

    fn record_output(&mut self, id: &str, message: &str, now_ms: u64) {
        if let Some(progress) = parse_build_progress(message) {
            let build = self.op_mut(id).build.get_or_insert_with(Default::default);
            observe_packages(build, &progress, now_ms);
            return;
        }
        for raw in message.lines() {
            let line = compact_log_line(raw);
            if line.is_empty() {
                continue;
            }
            if let Some((step, duration)) = parse_step_time(raw.trim()) {
                let step = LiveStep {
                    step,
                    ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                };
                push_capped(&mut self.op_mut(id).steps, step, STEP_CAPACITY);
            }
            let op = self.op_mut(id);
            if op.state == OpState::Running {
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

    fn end(&mut self, id: &str, state: OpState, now_ms: u64) {
        let op = self.op_mut(id);
        if op.started_ms.is_none() {
            op.started_ms = Some(now_ms);
        }
        op.state = state;
        op.ended_ms = Some(now_ms);
        op.last_log = None;
    }

    /// The entry of an operation, added at the end of the list when the plan
    /// did not name it.
    fn op_mut(&mut self, id: &str) -> &mut LiveOperation {
        let position = match self.index.get(id) {
            Some(position) => *position,
            None => {
                self.ops.push(LiveOperation {
                    id: id.to_string(),
                    domain: domain_of(id),
                    ..LiveOperation::default()
                });
                let position = self.ops.len() - 1;
                self.index.insert(id.to_string(), position);
                position
            }
        };
        &mut self.ops[position]
    }
}

/// The operations of a plan as a run starts: all pending, with the reason
/// each executes and the duration it took last time.
pub(crate) fn plan_operations<'a>(
    operations: impl IntoIterator<Item = &'a PlannedOperation>,
    durations_ms: &BTreeMap<String, u64>,
) -> Vec<LiveOperation> {
    operations
        .into_iter()
        .map(|operation| {
            let id = operation.id.as_str();
            let executes = operation.reuse.should_execute();
            let (reason_code, reason) = match &operation.reuse {
                OperationReuse::Execute(reason) => {
                    (Some(reason.code.to_string()), Some(reason.message.clone()))
                }
                OperationReuse::Reuse { source } => (None, Some(format!("reused from {source}"))),
            };
            LiveOperation {
                id: id.to_string(),
                domain: domain_of(id),
                state: OpState::Pending,
                planned_reuse: !executes,
                reason_code,
                reason,
                expected_ms: executes.then(|| durations_ms.get(id).copied()).flatten(),
                ..LiveOperation::default()
            }
        })
        .collect()
}

/// What a build runs for, as the dashboard header shows it: its target and
/// profile joined with a dot, or `None` when it names neither.
pub(crate) fn run_selection_label(spec: &ResolvedBuildSpec) -> Option<String> {
    let parts = [
        spec.metadata.target.as_deref(),
        spec.metadata.profile.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The variant a spec builds and its place among the declared variants, or
/// `None` for a build without variants.
pub(crate) fn live_variant(spec: &ResolvedBuildSpec) -> Option<LiveVariant> {
    let declared = &spec.selection.variants;
    if declared.is_empty() {
        return None;
    }
    let id = spec
        .selection
        .variant
        .clone()
        .unwrap_or_else(|| DEFAULT_VARIANT_ID.to_string());
    let index = declared
        .iter()
        .position(|variant| variant.id == id)
        .map_or(1, |position| position + 1);
    Some(LiveVariant {
        id,
        index,
        count: declared.len(),
    })
}

/// Folds one progress report into a Buildroot make's packages: a package that
/// is newly active starts its clock, one that left the active list is kept
/// with how long it was active.
fn observe_packages(build: &mut BuildProgressStatus, progress: &BuildProgress, now_ms: u64) {
    let mut active_since = Vec::with_capacity(progress.active.len());
    for name in &progress.active {
        let started_ms = build
            .active_since
            .iter()
            .find(|package| &package.name == name)
            .map_or(now_ms, |package| package.started_ms);
        active_since.push(PackageStart {
            name: name.clone(),
            started_ms,
        });
    }
    for previous in &build.active_since {
        if !progress.active.contains(&previous.name) {
            let timing = PackageTiming {
                name: previous.name.clone(),
                ms: now_ms.saturating_sub(previous.started_ms),
            };
            push_capped(&mut build.finished, timing, PACKAGE_CAPACITY);
        }
    }
    build.done = progress.done;
    build.total = progress.total;
    build.active = progress.active.clone();
    build.eta_secs = progress.eta.map(|eta| eta.as_secs());
    build.active_since = active_since;
}

fn push_capped<T>(list: &mut Vec<T>, item: T, capacity: usize) {
    if list.len() == capacity {
        list.remove(0);
    }
    list.push(item);
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_plan::OperationId;
    use std::time::Duration;

    fn id(value: &str) -> OperationId {
        OperationId::new(value)
    }

    fn tracker_with(ids: &[&str]) -> RunTracker {
        RunTracker::new(LiveRunInfo {
            build_name: "demo".into(),
            display_name: "Demo".into(),
            ops_total: ids.len(),
            operations: ids
                .iter()
                .map(|value| LiveOperation {
                    id: value.to_string(),
                    domain: domain_of(value),
                    expected_ms: Some(1_000),
                    ..LiveOperation::default()
                })
                .collect(),
            variant: None,
            selection: None,
        })
    }

    fn progress(done: usize, active: &[&str]) -> ExecutionEvent {
        ExecutionEvent::Log {
            operation_id: id("image:buildroot"),
            message: gaia_process::build_progress_message(&BuildProgress {
                done,
                total: 10,
                active: active.iter().map(|name| name.to_string()).collect(),
                eta: Some(Duration::from_secs(60)),
            }),
        }
    }

    #[test]
    fn operations_follow_their_events() {
        let mut tracker = tracker_with(&["source:a", "artifact:b", "image:c"]);
        tracker.record(
            &ExecutionEvent::Started {
                operation_id: id("source:a"),
            },
            1_000,
        );
        tracker.record(
            &ExecutionEvent::Succeeded {
                operation_id: id("source:a"),
            },
            3_500,
        );
        tracker.record(
            &ExecutionEvent::Reused {
                operation_id: id("artifact:b"),
            },
            3_600,
        );
        tracker.record(
            &ExecutionEvent::Started {
                operation_id: id("image:c"),
            },
            3_700,
        );
        let status = tracker.snapshot(false, 4_000);
        assert_eq!(status.ops_done, 2);
        assert_eq!(status.finished.done, 1);
        assert_eq!(status.finished.reused, 1);
        assert_eq!(status.running.len(), 1);
        assert_eq!(status.running[0].id, "image:c");

        let a = &status.operations[0];
        assert_eq!(a.state, OpState::Done);
        assert_eq!((a.started_ms, a.ended_ms), (Some(1_000), Some(3_500)));
        assert_eq!(a.domain, "source");
        assert_eq!(a.expected_ms, Some(1_000));
        assert_eq!(status.operations[1].state, OpState::Reused);
        assert_eq!(status.operations[2].state, OpState::Running);
    }

    #[test]
    fn unknown_operations_are_added_at_the_end() {
        let mut tracker = tracker_with(&["a:one"]);
        tracker.record(
            &ExecutionEvent::Failed {
                operation_id: id("late:two"),
                message: "boom".into(),
            },
            2_000,
        );
        let status = tracker.snapshot(false, 2_000);
        assert_eq!(status.operations.len(), 2);
        assert_eq!(status.operations[1].id, "late:two");
        assert_eq!(status.operations[1].state, OpState::Failed);
        assert_eq!(status.errors[0].message, "boom");
        assert_eq!(status.failed, vec!["late:two".to_string()]);
    }

    #[test]
    fn step_times_are_kept_per_operation() {
        let mut tracker = tracker_with(&["image:buildroot"]);
        tracker.record(
            &ExecutionEvent::Started {
                operation_id: id("image:buildroot"),
            },
            1_000,
        );
        tracker.record(
            &ExecutionEvent::Log {
                operation_id: id("image:buildroot"),
                message: gaia_process::step_time_message(
                    "buildroot make",
                    Duration::from_millis(61_250),
                ),
            },
            2_000,
        );
        let status = tracker.snapshot(false, 2_000);
        assert_eq!(
            status.operations[0].steps,
            vec![LiveStep {
                step: "buildroot make".into(),
                ms: 61_250,
            }]
        );
        // Step lines are still output, like any other line.
        assert_eq!(status.logs.len(), 1);
    }

    #[test]
    fn packages_keep_how_long_each_was_active() {
        let mut tracker = tracker_with(&["image:buildroot"]);
        tracker.record(&progress(1, &["linux", "mesa3d"]), 10_000);
        tracker.record(&progress(2, &["linux", "zlib"]), 14_000);
        tracker.record(&progress(3, &["zlib"]), 20_000);
        let build = tracker.snapshot(false, 20_000).operations[0]
            .build
            .clone()
            .expect("build progress");
        assert_eq!((build.done, build.total), (3, 10));
        assert_eq!(build.active, vec!["zlib".to_string()]);
        assert_eq!(build.eta_secs, Some(60));
        // mesa3d left after 4 s, linux after 10 s; zlib is still active.
        assert_eq!(
            build.finished,
            vec![
                PackageTiming {
                    name: "mesa3d".into(),
                    ms: 4_000,
                },
                PackageTiming {
                    name: "linux".into(),
                    ms: 10_000,
                },
            ]
        );
        assert_eq!(build.active_since[0].started_ms, 14_000);
    }

    #[test]
    fn plan_operations_start_pending_with_reasons_and_expectations() {
        use gaia_plan::{OperationKind, PlannedOperation};
        let mut run = PlannedOperation::new(id("artifact:cli"), OperationKind::ResolveBuild);
        run.reuse = OperationReuse::execute("source_changed", "the source changed");
        let mut reused = PlannedOperation::new(id("source:repo"), OperationKind::ResolveBuild);
        reused.reuse = OperationReuse::Reuse {
            source: "state".into(),
        };
        let durations = BTreeMap::from([
            ("artifact:cli".to_string(), 2_500),
            ("source:repo".to_string(), 90),
        ]);
        let operations = plan_operations([&run, &reused], &durations);
        assert_eq!(operations[0].state, OpState::Pending);
        assert_eq!(operations[0].reason_code.as_deref(), Some("source_changed"));
        assert_eq!(operations[0].expected_ms, Some(2_500));
        assert!(!operations[0].planned_reuse);
        assert!(operations[1].planned_reuse);
        assert_eq!(operations[1].expected_ms, None, "reuse has no duration");
    }
}
