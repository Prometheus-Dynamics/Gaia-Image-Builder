//! Tests of the run view: progress, estimates, curves and ordering, from
//! events and from snapshots.

use super::*;
use crate::commands::live_status::{BuildProgressStatus, RunningOperation, fixture_status};

/// A run of three operations: one done (2 s of an expected 1 s), one
/// running (its make is at 30 of 60 packages), one pending, plus one the
/// plan reuses.
pub(crate) fn fixture_view(now_ms: u64) -> RunView {
    let mut status = fixture_status();
    status.started_at = 1_000;
    status.operations = vec![
        LiveOperation {
            id: "source:repo".into(),
            domain: "source".into(),
            state: OpState::Reused,
            planned_reuse: true,
            reason: Some("reused from state".into()),
            started_ms: Some(1_000_000),
            ended_ms: Some(1_000_000),
            ..LiveOperation::default()
        },
        LiveOperation {
            id: "artifact:cli".into(),
            domain: "artifact".into(),
            state: OpState::Done,
            reason_code: Some("source_changed".into()),
            expected_ms: Some(1_000),
            started_ms: Some(1_000_000),
            ended_ms: Some(1_002_000),
            ..LiveOperation::default()
        },
        LiveOperation {
            id: "image:buildroot".into(),
            domain: "image".into(),
            state: OpState::Running,
            reason_code: Some("config_changed".into()),
            expected_ms: Some(120_000),
            started_ms: Some(1_010_000),
            build: Some(BuildProgressStatus {
                done: 30,
                total: 60,
                active: vec!["linux".into()],
                eta_secs: Some(90),
                ..BuildProgressStatus::default()
            }),
            steps: vec![crate::commands::live_status::LiveStep {
                step: "buildroot make".into(),
                ms: 60_000,
            }],
            ..LiveOperation::default()
        },
        LiveOperation {
            id: "image:assemble".into(),
            domain: "image".into(),
            state: OpState::Pending,
            reason_code: Some("config_changed".into()),
            expected_ms: Some(30_000),
            ..LiveOperation::default()
        },
    ];
    status.running = vec![RunningOperation {
        id: "image:buildroot".into(),
        started_at: 1_010,
        last_log: Some("building linux".into()),
        build: status.operations[2].build.clone(),
    }];
    status.operations[2].last_log = Some("building linux".into());
    status.finished = FinishedCounts {
        done: 1,
        reused: 0,
        ..FinishedCounts::default()
    };
    status.ops_total = 4;
    status.ops_done = 2;
    status.logs.push(LogLine {
        op: "image:buildroot".into(),
        line: "compiling linux".into(),
    });
    RunView::live(&status, RunSource::Monitor, false, now_ms)
}

#[test]
fn progress_is_weighted_by_expected_durations() {
    // Weights: artifact 1 s (done), buildroot 120 s at half (make 30/60),
    // assemble 30 s (pending). Reused weighs nothing.
    let view = fixture_view(1_060_000);
    let done = 1_000.0 + 0.5 * 120_000.0;
    let total = 1_000.0 + 120_000.0 + 30_000.0;
    assert!((view.progress() - done / total).abs() < 1e-9);
    assert_eq!(
        view.progress_percent(),
        (done / total * 100.0).floor() as u64
    );
}

#[test]
fn progress_falls_back_to_counts_without_durations() {
    let mut view = fixture_view(1_060_000);
    for op in &mut view.ops {
        op.expected_ms = None;
    }
    // Without durations every executing op weighs the same: 1.5 of 3
    // (one done, one half-way through its make, one pending).
    assert!((view.progress() - 0.5).abs() < 1e-9);
}

#[test]
fn eta_extrapolates_from_progress_once_moving() {
    let view = fixture_view(1_060_000);
    let progress = view.progress();
    let elapsed = view.elapsed_ms() as f64;
    let eta = view.eta_ms().expect("eta") as f64;
    assert!((eta - elapsed * (1.0 - progress) / progress).abs() < 2.0);
}

#[test]
fn delta_compares_the_duration_with_the_expected_one() {
    let view = fixture_view(1_060_000);
    // artifact:cli took 2 s, expected 1 s.
    assert_eq!(view.delta_ms(&view.ops[1]), Some(1_000));
    // buildroot has run 50 s so far, expected 120 s.
    assert_eq!(view.delta_ms(&view.ops[2]), Some(-70_000));
    // assemble has not started.
    assert_eq!(view.delta_ms(&view.ops[3]), None);
}

#[test]
fn display_order_filters_and_sorts() {
    let view = fixture_view(1_060_000);
    assert_eq!(
        view.display_order(OpFilter::All, OpSort::Plan),
        vec![0, 1, 2, 3]
    );
    assert_eq!(view.display_order(OpFilter::Running, OpSort::Plan), vec![2]);
    assert_eq!(view.display_order(OpFilter::Slow, OpSort::Plan), vec![1]);
    assert_eq!(
        view.display_order(OpFilter::Failed, OpSort::Plan),
        Vec::<usize>::new()
    );
    // Longest first: buildroot (50 s) before artifact (2 s); unknown last.
    assert_eq!(
        view.display_order(OpFilter::All, OpSort::Duration)[..2],
        [2, 1]
    );
}

#[test]
fn curves_compare_this_run_with_the_last_one() {
    let view = fixture_view(1_060_000);
    let this = view.curve_this_run();
    assert_eq!(this.first(), Some(&(0.0, 0.0)));
    // artifact finished at 2 s, a small share of the weight.
    assert!(
        this.iter()
            .any(|(seconds, percent)| (seconds - 2.0).abs() < 1e-9
                && *percent > 0.0
                && *percent < 1.0)
    );
    // The present: 60 s in, with the progress so far.
    let present = this.last().copied().expect("present");
    assert!((present.0 - 60.0).abs() < 1e-9);
    assert!((present.1 - view.progress() * 100.0).abs() < 1e-9);

    let last = view.curve_last_run();
    // Replayed in plan order: artifact 1 s, buildroot 120 s, assemble 30 s.
    assert_eq!(last.len(), 4);
    assert!((last[3].0 - 151.0).abs() < 1e-9);
    assert!((last[3].1 - 100.0).abs() < 1e-9);
}

#[test]
fn slowest_lists_finished_work_longest_first() {
    let view = fixture_view(1_060_000);
    let slowest = view.slowest(5);
    assert_eq!(slowest.len(), 1);
    assert_eq!(slowest[0].0.id, "artifact:cli");
    assert_eq!(slowest[0].1, 2_000);
}

#[test]
fn package_op_is_the_running_make() {
    let view = fixture_view(1_060_000);
    assert_eq!(
        view.package_op().map(|op| op.id.as_str()),
        Some("image:buildroot")
    );
    assert_eq!(
        view.current_op().map(|op| op.id.as_str()),
        Some("image:buildroot")
    );
}

#[test]
fn a_finished_run_has_no_eta_and_reads_its_outcome() {
    let status = fixture_status();
    let view = RunView::ended(
        &status,
        "failed",
        1_200_000,
        RunSource::Monitor,
        &[],
        1_300_000,
    );
    assert_eq!(view.phase, RunPhase::Failed);
    assert_eq!(view.eta_ms(), None);
    assert_eq!(view.elapsed_ms(), 200_000);
    let cancelled = RunView::ended(&status, "cancelled", 1_200_000, RunSource::Attached, &[], 1);
    assert_eq!(cancelled.phase, RunPhase::Cancelled);
    let unknown = RunView::ended(&status, "weird", 1_200_000, RunSource::Attached, &[], 1);
    assert_eq!(unknown.phase, RunPhase::Ended);
}

#[test]
fn a_snapshot_from_an_older_gaia_lists_its_running_operations() {
    let status = fixture_status();
    let view = RunView::live(&status, RunSource::Attached, true, 1_100_000);
    assert_eq!(view.phase, RunPhase::Paused);
    assert_eq!(view.ops.len(), 1);
    assert_eq!(view.ops[0].state, OpState::Running);
    assert_eq!(view.ops[0].domain, "image");
}

#[test]
fn a_run_fed_by_events_reads_the_same_as_its_snapshot_on_disk() {
    use crate::commands::live_status::{LiveRunInfo, RunTracker};
    use gaia_exec::ExecutionEvent;
    use gaia_plan::OperationId;
    use gaia_process::{BuildProgress, build_progress_message};
    use std::time::Duration;

    let planned = |id: &str, expected: u64| LiveOperation {
        id: id.into(),
        domain: crate::commands::live_status::domain_of(id),
        expected_ms: Some(expected),
        ..LiveOperation::default()
    };
    let mut tracker = RunTracker::new(LiveRunInfo {
        build_name: "demo".into(),
        display_name: "Demo".into(),
        ops_total: 2,
        operations: vec![
            planned("artifact:cli", 1_000),
            planned("image:buildroot", 60_000),
        ],
        variant: None,
        selection: None,
    });
    let id = OperationId::new;
    tracker.record(
        &ExecutionEvent::Started {
            operation_id: id("artifact:cli"),
        },
        1_000_000,
    );
    tracker.record(
        &ExecutionEvent::Succeeded {
            operation_id: id("artifact:cli"),
        },
        1_002_000,
    );
    tracker.record(
        &ExecutionEvent::Started {
            operation_id: id("image:buildroot"),
        },
        1_002_000,
    );
    tracker.record(
        &ExecutionEvent::Log {
            operation_id: id("image:buildroot"),
            message: build_progress_message(&BuildProgress {
                done: 30,
                total: 60,
                active: vec!["linux".into()],
                eta: Some(Duration::from_secs(30)),
            }),
        },
        1_010_000,
    );
    let status = tracker.snapshot(false, 1_030_000);
    let monitor = RunView::live(&status, RunSource::Monitor, false, 1_030_000);
    assert_eq!(monitor.counts.done, 1);
    assert_eq!(
        monitor.current_op().map(|op| op.id.as_str()),
        Some("image:buildroot")
    );
    assert_eq!(
        monitor.op("artifact:cli").map(|op| op.state),
        Some(OpState::Done)
    );
    // The buildroot make is half-way through its packages: the progress
    // is the finished 1 s plus half of the 60 s make, over 61 s.
    let expected = (1_000.0 + 0.5 * 60_000.0) / 61_000.0;
    assert!((monitor.progress() - expected).abs() < 1e-9);

    // The attached view of the same run, read back from its file.
    let json = serde_json::to_string(&status).expect("encode");
    let decoded: LiveStatus = serde_json::from_str(&json).expect("decode");
    let attached = RunView::live(&decoded, RunSource::Attached, false, 1_030_000);
    assert_eq!(attached.ops, monitor.ops);
    assert_eq!(attached.progress(), monitor.progress());
    assert_eq!(attached.counts, monitor.counts);
}

#[test]
fn starting_run_has_no_progress_yet() {
    let mut status = fixture_status();
    status.running.clear();
    status.operations.clear();
    status.finished = FinishedCounts::default();
    let view = RunView::live(&status, RunSource::Attached, false, 1_000_000);
    assert_eq!(view.phase, RunPhase::Starting);
    assert_eq!(view.progress(), 0.0);
}
