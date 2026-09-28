use super::*;
use gaia_plan::{OperationId, PlanEstimate};
use std::time::Duration;

#[test]
fn plan_estimate_lines_report_critical_path_and_untimed_operations() {
    let estimate = PlanEstimate {
        total_work: Duration::from_secs(3_900),
        critical_path: vec![
            OperationId::new("source:a"),
            OperationId::new("image:build"),
        ],
        critical_path_duration: Duration::from_secs(3_720),
        timed_operations: 2,
        untimed_operations: vec![OperationId::new("artifact:new")],
    };
    assert_eq!(
        plan_estimate_lines(&estimate),
        vec![
            "plan estimate: critical-path=1h02m total-work=1h05m timed=2/3".to_string(),
            "plan critical path: source:a -> image:build".to_string(),
            "plan estimate excludes untimed: artifact:new".to_string(),
        ]
    );
    assert_eq!(
        plan_estimate_lines(&PlanEstimate::default()),
        vec!["plan estimate: nothing to execute".to_string()]
    );
}

#[test]
fn slowest_operations_skip_reused_and_sort_descending() {
    let record = |id: &str, ms: u64, status: &str| gaia_report::OperationTimingRecord {
        operation_id: id.into(),
        duration_ms: ms,
        status: status.into(),
    };
    let lines = slowest_operation_lines(
        &[
            record("a", 5_000, "built"),
            record("b", 90_000, "reused"),
            record("c", 65_000, "failed"),
        ],
        5,
    );
    assert_eq!(
        lines,
        vec![
            "operation time: 1m05s c (failed)".to_string(),
            "operation time: 5s a (built)".to_string(),
        ]
    );
}
