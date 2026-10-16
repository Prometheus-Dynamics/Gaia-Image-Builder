//! The model the run dashboard draws: one run, as the live status file
//! describes it. The in-process monitor and an attached run both become a
//! [`RunView`] (the monitor through a `RunTracker` fed by execution events),
//! so the dashboard does not care where the run lives.
//!
//! Times are unix milliseconds. Progress is weighted by the expected duration
//! of each operation (its duration in the last run), so a long Buildroot make
//! counts for more than a checksum. Operations the plan reuses carry no
//! weight: they are already done.

use std::cmp::Ordering;

use crate::commands::live_status::{
    FinishedCounts, LastRun, LiveOperation, LiveStatus, LiveVariant, LogLine, OpState, RunError,
};

/// Whether the dashboard shows the monitor's run or one another process runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunSource {
    Monitor,
    Attached,
}

/// Where the run stands, as the header shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunPhase {
    /// Started, nothing has reported yet.
    Starting,
    Running,
    Paused,
    /// Cancellation was asked for and the run has not stopped yet.
    Cancelling,
    Completed,
    Failed,
    Cancelled,
    /// Not live, and the outcome is unknown.
    Ended,
}

impl RunPhase {
    fn from_outcome(outcome: &str) -> Self {
        match outcome {
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => Self::Ended,
        }
    }

    pub(crate) fn is_over(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Ended
        )
    }
}

/// An image the run published, with its sizes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImageSize {
    pub path: String,
    pub raw_bytes: u64,
    pub content_bytes: u64,
}

/// What only a run in this process knows once it has finished: its report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RunOutcome {
    pub image_sizes: Vec<ImageSize>,
    pub out_dir: Option<String>,
    pub report_files: usize,
}

#[derive(Clone)]
pub(crate) struct RunView {
    pub source: RunSource,
    pub pid: Option<u32>,
    pub display_name: String,
    pub phase: RunPhase,
    pub variant: Option<LiveVariant>,
    /// What the run builds for: `target · profile`.
    pub selection: Option<String>,
    pub started_ms: u64,
    /// Set once the run is over.
    pub ended_ms: Option<u64>,
    pub now_ms: u64,
    pub ops_total: usize,
    /// Every planned operation, in plan order.
    pub ops: Vec<LiveOperation>,
    pub counts: FinishedCounts,
    pub errors: Vec<RunError>,
    /// Recent output, oldest first.
    pub logs: Vec<LogLine>,
    pub outcome: RunOutcome,
}

/// The filter of the operations table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum OpFilter {
    #[default]
    All,
    Running,
    Failed,
    /// Ran longer than expected.
    Slow,
}

impl OpFilter {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::All => Self::Running,
            Self::Running => Self::Failed,
            Self::Failed => Self::Slow,
            Self::Slow => Self::All,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Running => "running",
            Self::Failed => "failed",
            Self::Slow => "slow",
        }
    }
}

/// The order of the operations table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum OpSort {
    #[default]
    Plan,
    /// Longest first.
    Duration,
    /// Furthest over expectation first.
    Delta,
}

impl OpSort {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Plan => Self::Duration,
            Self::Duration => Self::Delta,
            Self::Delta => Self::Plan,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Plan => "plan order",
            Self::Duration => "duration",
            Self::Delta => "delta",
        }
    }
}

impl RunView {
    /// A run that is live, from its status snapshot.
    pub(crate) fn live(status: &LiveStatus, source: RunSource, paused: bool, now_ms: u64) -> Self {
        let mut view = Self::from_status(status, source, now_ms);
        view.phase = if paused {
            RunPhase::Paused
        } else if view.counts.total() == 0 && view.ops.iter().all(|op| op.started_ms.is_none()) {
            RunPhase::Starting
        } else {
            RunPhase::Running
        };
        view
    }

    /// A run that is over, from its final snapshot and outcome.
    pub(crate) fn ended(
        status: &LiveStatus,
        outcome: &str,
        ended_ms: u64,
        source: RunSource,
        errors: &[RunError],
        now_ms: u64,
    ) -> Self {
        let mut view = Self::from_status(status, source, now_ms);
        view.phase = RunPhase::from_outcome(outcome);
        view.ended_ms = Some(ended_ms);
        if !errors.is_empty() {
            view.errors = errors.to_vec();
        }
        view
    }

    /// A run from its final snapshot read off disk.
    pub(crate) fn from_last(last: &LastRun, source: RunSource, now_ms: u64) -> Self {
        let ended_ms = last.ended_at.saturating_mul(1000);
        Self::ended(
            &last.status,
            &last.outcome,
            ended_ms,
            source,
            &last.errors,
            now_ms,
        )
    }

    /// A run that has started but has not written a snapshot yet.
    pub(crate) fn starting(pid: Option<u32>, source: RunSource, now_ms: u64) -> Self {
        Self {
            source,
            pid,
            display_name: "starting".into(),
            phase: RunPhase::Starting,
            variant: None,
            selection: None,
            started_ms: now_ms,
            ended_ms: None,
            now_ms,
            ops_total: 0,
            ops: Vec::new(),
            counts: FinishedCounts::default(),
            errors: Vec::new(),
            logs: Vec::new(),
            outcome: RunOutcome::default(),
        }
    }

    fn from_status(status: &LiveStatus, source: RunSource, now_ms: u64) -> Self {
        let started_ms = status.started_at.saturating_mul(1000);
        Self {
            source,
            pid: Some(status.pid),
            display_name: status.display_name.clone(),
            phase: RunPhase::Running,
            variant: status.variant.clone(),
            selection: status.selection.clone(),
            started_ms,
            ended_ms: None,
            now_ms: now_ms.max(started_ms),
            ops_total: status.ops_total,
            ops: status.operations.clone(),
            counts: status.finished.clone(),
            errors: status.errors.clone(),
            logs: status.logs.clone(),
            outcome: RunOutcome::default(),
        }
        .with_running_fallback(status)
    }

    /// Operations a snapshot from an older Gaia lists only as running.
    fn with_running_fallback(mut self, status: &LiveStatus) -> Self {
        if !self.ops.is_empty() {
            return self;
        }
        for running in &status.running {
            self.ops.push(LiveOperation {
                id: running.id.clone(),
                domain: crate::commands::live_status::domain_of(&running.id),
                state: OpState::Running,
                started_ms: Some(running.started_at.saturating_mul(1000)),
                last_log: running.last_log.clone(),
                build: running.build.clone(),
                ..LiveOperation::default()
            });
        }
        self
    }

    pub(crate) fn with_outcome(mut self, outcome: RunOutcome) -> Self {
        self.outcome = outcome;
        self
    }

    pub(crate) fn is_over(&self) -> bool {
        self.phase.is_over()
    }

    /// Time since the run started, to its end once it is over.
    pub(crate) fn elapsed_ms(&self) -> u64 {
        self.ended_ms
            .unwrap_or(self.now_ms)
            .saturating_sub(self.started_ms)
    }

    /// The time an operation has run (to its end, or to now while it runs).
    pub(crate) fn op_duration_ms(&self, op: &LiveOperation) -> Option<u64> {
        let start = op.started_ms?;
        Some(op.ended_ms.unwrap_or(self.now_ms).saturating_sub(start))
    }

    pub(crate) fn op(&self, id: &str) -> Option<&LiveOperation> {
        self.ops.iter().find(|op| op.id == id)
    }

    /// The running operation that started last: what the run is doing now.
    pub(crate) fn current_op(&self) -> Option<&LiveOperation> {
        self.ops
            .iter()
            .filter(|op| op.state == OpState::Running)
            .max_by_key(|op| op.started_ms.unwrap_or(0))
    }

    /// The operation with Buildroot package progress: the running one, or the
    /// last one that reported it.
    pub(crate) fn package_op(&self) -> Option<&LiveOperation> {
        self.current_op()
            .filter(|op| op.build.is_some())
            .or_else(|| {
                self.ops
                    .iter()
                    .rev()
                    .find(|op| op.build.as_ref().is_some_and(|build| build.total > 0))
            })
    }

    /// The operations that will execute (not planned as reuse).
    fn executing(&self) -> impl Iterator<Item = &LiveOperation> {
        self.ops.iter().filter(|op| !op.planned_reuse)
    }

    /// The weight of an operation in the overall progress: its expected
    /// duration, or the average of the known ones when it has none.
    pub(crate) fn weight(&self, op: &LiveOperation) -> f64 {
        if op.planned_reuse {
            return 0.0;
        }
        match op.expected_ms {
            Some(ms) => ms as f64,
            None => self.fallback_weight(),
        }
    }

    fn fallback_weight(&self) -> f64 {
        let known = self
            .executing()
            .filter_map(|op| op.expected_ms)
            .collect::<Vec<_>>();
        if known.is_empty() {
            1.0
        } else {
            known.iter().sum::<u64>() as f64 / known.len() as f64
        }
    }

    /// How far an operation has got, from 0 to 1.
    pub(crate) fn fraction(&self, op: &LiveOperation) -> f64 {
        match op.state {
            OpState::Pending | OpState::Unknown => 0.0,
            OpState::Done
            | OpState::Reused
            | OpState::Failed
            | OpState::Cancelled
            | OpState::Skipped => 1.0,
            OpState::Running => {
                if let Some(build) = op.build.as_ref().filter(|build| build.total > 0) {
                    return (build.done as f64 / build.total as f64).min(1.0);
                }
                match (op.expected_ms, self.op_duration_ms(op)) {
                    (Some(expected), Some(elapsed)) if expected > 0 => {
                        (elapsed as f64 / expected as f64).min(0.95)
                    }
                    _ => 0.0,
                }
            }
        }
    }

    /// The overall progress, from 0 to 1, weighted by expected durations.
    pub(crate) fn progress(&self) -> f64 {
        let total: f64 = self.ops.iter().map(|op| self.weight(op)).sum();
        if total <= 0.0 {
            return if self.ops_total == 0 {
                0.0
            } else {
                (self.counts.total().min(self.ops_total) as f64) / self.ops_total as f64
            };
        }
        let done: f64 = self
            .ops
            .iter()
            .map(|op| self.weight(op) * self.fraction(op))
            .sum();
        (done / total).clamp(0.0, 1.0)
    }

    pub(crate) fn progress_percent(&self) -> u64 {
        if self.is_over() && self.phase == RunPhase::Completed {
            return 100;
        }
        (self.progress() * 100.0).floor() as u64
    }

    /// The time left, extrapolated from the progress so far (which accounts
    /// for parallel operations). Before any progress, the expected durations
    /// of what is left.
    pub(crate) fn eta_ms(&self) -> Option<u64> {
        if self.is_over() {
            return None;
        }
        let progress = self.progress();
        let elapsed = self.elapsed_ms() as f64;
        if (0.02..1.0).contains(&progress) && elapsed > 0.0 {
            return Some((elapsed * (1.0 - progress) / progress) as u64);
        }
        let remaining: u64 = self
            .executing()
            .filter(|op| matches!(op.state, OpState::Pending | OpState::Running))
            .map(|op| {
                let expected = op.expected_ms.unwrap_or(0);
                expected.saturating_sub(self.op_duration_ms(op).unwrap_or(0))
            })
            .sum();
        (remaining > 0).then_some(remaining)
    }

    /// The recorded duration of the whole run: the sum of the last durations of
    /// what executes, when any is known.
    pub(crate) fn expected_total_ms(&self) -> Option<u64> {
        let known = self
            .executing()
            .filter_map(|op| op.expected_ms)
            .collect::<Vec<_>>();
        (!known.is_empty()).then(|| known.iter().sum())
    }

    /// Time over (positive) or under (negative) what the operation took last
    /// time. A running operation is compared as it is now.
    pub(crate) fn delta_ms(&self, op: &LiveOperation) -> Option<i64> {
        let expected = op.expected_ms?;
        let actual = self.op_duration_ms(op)?;
        Some(actual as i64 - expected as i64)
    }

    /// The operations that finished, longest first, without reused ones.
    pub(crate) fn slowest(&self, count: usize) -> Vec<(&LiveOperation, u64)> {
        let mut finished = self
            .ops
            .iter()
            .filter(|op| {
                !op.planned_reuse
                    && matches!(
                        op.state,
                        OpState::Done | OpState::Failed | OpState::Cancelled
                    )
            })
            .filter_map(|op| Some((op, self.op_duration_ms(op)?)))
            .collect::<Vec<_>>();
        finished.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.id.cmp(&b.0.id)));
        finished.truncate(count);
        finished
    }

    /// Cumulative progress against elapsed time, as `(seconds, percent)`
    /// points: one per finished operation, then the present.
    pub(crate) fn curve_this_run(&self) -> Vec<(f64, f64)> {
        let total: f64 = self.ops.iter().map(|op| self.weight(op)).sum();
        if total <= 0.0 {
            return Vec::new();
        }
        let mut ended = self
            .ops
            .iter()
            .filter_map(|op| Some((op.ended_ms?, self.weight(op))))
            .collect::<Vec<_>>();
        ended.sort_by_key(|(at, _)| *at);
        let mut points = vec![(0.0, 0.0)];
        let mut done = 0.0;
        for (at, weight) in ended {
            done += weight;
            let seconds = at.saturating_sub(self.started_ms) as f64 / 1000.0;
            points.push((seconds, done / total * 100.0));
        }
        points.push((self.elapsed_ms() as f64 / 1000.0, self.progress() * 100.0));
        points
    }

    /// The last run replayed in plan order from its recorded durations, as
    /// `(seconds, percent)` points. Empty when no duration was recorded.
    pub(crate) fn curve_last_run(&self) -> Vec<(f64, f64)> {
        let known = self.executing().any(|op| op.expected_ms.is_some());
        if !known {
            return Vec::new();
        }
        let total: f64 = self.ops.iter().map(|op| self.weight(op)).sum();
        if total <= 0.0 {
            return Vec::new();
        }
        let mut points = vec![(0.0, 0.0)];
        let mut time = 0.0;
        let mut done = 0.0;
        for op in self.executing() {
            time += op.expected_ms.unwrap_or(0) as f64;
            done += self.weight(op);
            points.push((time / 1000.0, done / total * 100.0));
        }
        points
    }

    /// The operations as the table shows them: filtered, then sorted, as
    /// indices into [`ops`](Self::ops).
    pub(crate) fn display_order(&self, filter: OpFilter, sort: OpSort) -> Vec<usize> {
        let mut indices = (0..self.ops.len())
            .filter(|&index| self.passes(&self.ops[index], filter))
            .collect::<Vec<_>>();
        match sort {
            OpSort::Plan => {}
            OpSort::Duration => indices.sort_by(|&a, &b| {
                let (a, b) = (&self.ops[a], &self.ops[b]);
                let key = |op: &LiveOperation| self.op_duration_ms(op).map(|ms| ms as i64);
                cmp_desc(key(a), key(b))
            }),
            OpSort::Delta => indices.sort_by(|&a, &b| {
                let (a, b) = (&self.ops[a], &self.ops[b]);
                cmp_desc(self.delta_ms(a), self.delta_ms(b))
            }),
        }
        indices
    }

    fn passes(&self, op: &LiveOperation, filter: OpFilter) -> bool {
        match filter {
            OpFilter::All => true,
            OpFilter::Running => op.state == OpState::Running,
            OpFilter::Failed => matches!(op.state, OpState::Failed | OpState::Cancelled),
            OpFilter::Slow => self.delta_ms(op).is_some_and(|delta| delta > 0),
        }
    }

    /// Output lines of one operation, oldest first, from the recent output.
    pub(crate) fn op_log_lines(&self, op_id: &str) -> Vec<String> {
        self.logs
            .iter()
            .filter(|log| log.op == op_id)
            .map(|log| log.line.clone())
            .collect()
    }
}

/// Descending order with unknown values last.
fn cmp_desc<T: Ord>(a: Option<T>, b: Option<T>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => b.cmp(&a),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
pub(crate) mod tests;
