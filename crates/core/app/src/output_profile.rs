//! Timing lines of the command output: the plan estimate, and after a run
//! its critical path and slowest operations and steps.

/// `gaia plan` estimate lines: total work and the critical path, from the
/// durations recorded by earlier runs.
pub(crate) fn plan_estimate_lines(estimate: &gaia_plan::PlanEstimate) -> Vec<String> {
    use gaia_plan::format_duration_short;
    if estimate.executing_operations() == 0 {
        return vec!["plan estimate: nothing to execute".into()];
    }
    if estimate.timed_operations == 0 {
        return vec![format!(
            "plan estimate: unknown (no recorded timings for the {} operation(s) that will execute)",
            estimate.executing_operations()
        )];
    }
    let mut lines = vec![format!(
        "plan estimate: critical-path={} total-work={} timed={}/{}",
        format_duration_short(estimate.critical_path_duration),
        format_duration_short(estimate.total_work),
        estimate.timed_operations,
        estimate.executing_operations(),
    )];
    if !estimate.critical_path.is_empty() {
        lines.push(format!(
            "plan critical path: {}",
            estimate
                .critical_path
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>()
                .join(" -> ")
        ));
    }
    if !estimate.untimed_operations.is_empty() {
        lines.push(format!(
            "plan estimate excludes untimed: {}",
            estimate
                .untimed_operations
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    lines
}

/// The `limit` slowest operations that actually ran, slowest first.
pub(crate) fn slowest_operation_lines(
    timings: &[gaia_report::OperationTimingRecord],
    limit: usize,
) -> Vec<String> {
    let mut ran = timings
        .iter()
        .filter(|timing| timing.status != "reused")
        .collect::<Vec<_>>();
    ran.sort_by_key(|timing| std::cmp::Reverse(timing.duration_ms));
    ran.into_iter()
        .take(limit)
        .map(|timing| {
            let paused = if timing.paused_ms > 0 {
                format!(
                    ", paused {}",
                    gaia_plan::format_duration_short(std::time::Duration::from_millis(
                        timing.paused_ms
                    ))
                )
            } else {
                String::new()
            };
            format!(
                "operation time: {} {} ({}{paused})",
                gaia_plan::format_duration_short(std::time::Duration::from_millis(
                    timing.duration_ms
                )),
                timing.operation_id,
                timing.status
            )
        })
        .collect()
}

/// The `limit` slowest steps inside operations, slowest first, so a run
/// shows where its time went (the Buildroot make, a package cache restore,
/// an assembly step, the slowest Buildroot packages).
pub(crate) fn slowest_step_lines(
    timings: &[gaia_report::OperationTimingRecord],
    limit: usize,
) -> Vec<String> {
    let mut steps = timings
        .iter()
        .flat_map(|timing| {
            timing
                .steps
                .iter()
                .map(move |step| (step.duration_ms, &timing.operation_id, &step.step))
        })
        .collect::<Vec<_>>();
    steps.sort_by_key(|(duration_ms, _, _)| std::cmp::Reverse(*duration_ms));
    steps
        .into_iter()
        .take(limit)
        .map(|(duration_ms, operation_id, step)| {
            format!(
                "step time: {} {operation_id} > {step}",
                gaia_plan::format_duration_short(std::time::Duration::from_millis(duration_ms))
            )
        })
        .collect()
}

/// After a run: the chain of operations that decided its length, e.g.
/// `critical path 9m40s: source:eidos 5s → artifact:api 3m18s → image:assembly 58s`.
/// Long paths keep their first and last operations.
pub(crate) fn critical_path_line(path: &gaia_report::CriticalPathRecord) -> String {
    const SHOWN: usize = 4;
    let short = |ms: u64| gaia_plan::format_duration_short(std::time::Duration::from_millis(ms));
    let entry = |entry: &gaia_report::CriticalPathEntry| {
        format!("{} {}", entry.operation_id, short(entry.duration_ms))
    };
    let operations = &path.operations;
    let mut parts = Vec::new();
    if operations.len() > SHOWN * 2 + 1 {
        parts.extend(operations[..SHOWN].iter().map(entry));
        parts.push(format!("… {} more", operations.len() - SHOWN * 2));
        parts.extend(operations[operations.len() - SHOWN..].iter().map(entry));
    } else {
        parts.extend(operations.iter().map(entry));
    }
    format!(
        "critical path {}: {}",
        short(path.duration_ms),
        parts.join(" → ")
    )
}
