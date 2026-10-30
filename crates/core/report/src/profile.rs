//! The run profile: the actual critical path of a run, from the recorded
//! operation start and finish times and the plan's dependencies, and the
//! run as Chrome trace events (`<build>.trace.json`, for chrome://tracing
//! or Perfetto).

use gaia_plan::{ExecutionPlan, OperationId};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};

use crate::model::{CriticalPathEntry, CriticalPathRecord, OperationTimingRecord, RunSummary};

/// Steps with this prefix run one after another inside the assembly
/// operation, so the trace can show them as slices inside it.
const ASSEMBLY_STEP_PREFIX: &str = "assembly ";

/// The chain of operations that decided the run's length: it ends at the
/// operation that finished last and steps back, each time, to the dependency
/// that finished last. Reused operations take no time and are passed through
/// to their own dependencies. `None` when no operation ran.
pub fn critical_path(
    plan: &ExecutionPlan,
    timings: &[OperationTimingRecord],
) -> Option<CriticalPathRecord> {
    let ran = timings
        .iter()
        .filter(|timing| timing.status != "reused")
        .map(|timing| (timing.operation_id.as_str(), timing))
        .collect::<HashMap<_, _>>();
    let dependencies = plan
        .operations
        .iter()
        .map(|operation| (operation.id.as_str(), operation.depends_on.as_slice()))
        .collect::<HashMap<_, _>>();
    let mut current = latest(ran.values().copied())?;
    let mut path = vec![current];
    while let Some(previous) = latest(
        ran_dependencies(&current.operation_id, &dependencies, &ran)
            .into_iter()
            .filter(|timing| !path.contains(timing)),
    ) {
        path.push(previous);
        current = previous;
    }
    path.reverse();
    let first_start = path.first().map_or(0, |timing| timing.start_ms);
    let last_end = path.last().map_or(0, |timing| timing.end_ms());
    let paused = path.iter().map(|timing| timing.paused_ms).sum::<u64>();
    Some(CriticalPathRecord {
        duration_ms: last_end.saturating_sub(first_start).saturating_sub(paused),
        operations: path
            .into_iter()
            .map(|timing| CriticalPathEntry {
                operation_id: timing.operation_id.clone(),
                start_ms: timing.start_ms,
                duration_ms: timing.duration_ms,
                status: timing.status.clone(),
            })
            .collect(),
    })
}

/// The operation that finished last; ties go to the smaller id so the path
/// is stable.
fn latest<'a>(
    candidates: impl Iterator<Item = &'a OperationTimingRecord>,
) -> Option<&'a OperationTimingRecord> {
    candidates.max_by(|a, b| {
        a.end_ms()
            .cmp(&b.end_ms())
            .then_with(|| b.operation_id.cmp(&a.operation_id))
    })
}

/// The nearest dependencies of `id` that ran, looking through dependencies
/// that were reused or have no timing.
fn ran_dependencies<'a>(
    id: &str,
    dependencies: &HashMap<&str, &'a [OperationId]>,
    ran: &HashMap<&str, &'a OperationTimingRecord>,
) -> Vec<&'a OperationTimingRecord> {
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    let mut stack = dependencies
        .get(id)
        .map_or(Vec::new(), |deps| deps.iter().collect::<Vec<_>>());
    while let Some(dependency) = stack.pop() {
        if !seen.insert(dependency.as_str()) {
            continue;
        }
        if let Some(timing) = ran.get(dependency.as_str()) {
            found.push(*timing);
        } else if let Some(deps) = dependencies.get(dependency.as_str()) {
            stack.extend(deps.iter());
        }
    }
    found
}

/// The run as a Chrome trace (JSON object format): one complete (`X`) event
/// per operation, on rows that never overlap, and the assembly steps as
/// slices inside the assembly operation. Times are microseconds since
/// execution began.
pub(crate) fn chrome_trace(summary: &RunSummary) -> Value {
    let critical = summary
        .critical_path
        .iter()
        .flat_map(|path| path.operations.iter())
        .map(|entry| entry.operation_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut timings = summary.operation_timings.iter().collect::<Vec<_>>();
    timings.sort_by(|a, b| {
        a.start_ms
            .cmp(&b.start_ms)
            .then_with(|| a.operation_id.cmp(&b.operation_id))
    });
    let mut events = vec![json!({
        "name": "process_name",
        "ph": "M",
        "pid": 1,
        "args": { "name": format!("gaia run {}", summary.build_name) },
    })];
    // Each row's end, so an operation goes on the first row free at its start.
    let mut rows: Vec<u64> = Vec::new();
    for timing in timings {
        let row = match rows.iter().position(|end| *end <= timing.start_ms) {
            Some(row) => row,
            None => {
                rows.push(0);
                rows.len() - 1
            }
        };
        rows[row] = timing.end_ms().max(timing.start_ms + 1);
        let other_steps = timing
            .steps
            .iter()
            .filter(|step| !step.step.starts_with(ASSEMBLY_STEP_PREFIX))
            .map(|step| json!({ "step": step.step, "duration_ms": step.duration_ms }))
            .collect::<Vec<_>>();
        let mut args = json!({
            "status": timing.status,
            "duration_ms": timing.duration_ms,
            "critical_path": critical.contains(timing.operation_id.as_str()),
        });
        if timing.paused_ms > 0 {
            args["paused_ms"] = json!(timing.paused_ms);
        }
        if !other_steps.is_empty() {
            args["steps"] = Value::Array(other_steps);
        }
        events.push(json!({
            "name": timing.operation_id,
            "cat": category(&timing.operation_id),
            "ph": "X",
            "ts": timing.start_ms * 1000,
            "dur": (timing.end_ms() - timing.start_ms) * 1000,
            "pid": 1,
            "tid": row + 1,
            "args": args,
        }));
        // Only their durations are recorded: laid end to end from the
        // operation's start, and kept inside it.
        let mut step_start = timing.start_ms;
        for step in timing
            .steps
            .iter()
            .filter(|step| step.step.starts_with(ASSEMBLY_STEP_PREFIX))
        {
            let duration = step
                .duration_ms
                .min(timing.end_ms().saturating_sub(step_start));
            events.push(json!({
                "name": step.step.trim_start_matches(ASSEMBLY_STEP_PREFIX),
                "cat": "assembly-step",
                "ph": "X",
                "ts": step_start * 1000,
                "dur": duration * 1000,
                "pid": 1,
                "tid": row + 1,
                "args": { "duration_ms": step.duration_ms },
            }));
            step_start += duration;
        }
    }
    for row in 0..rows.len() {
        events.push(json!({
            "name": "thread_name",
            "ph": "M",
            "pid": 1,
            "tid": row + 1,
            "args": { "name": format!("row {}", row + 1) },
        }));
    }
    json!({
        "traceEvents": events,
        "displayTimeUnit": "ms",
        "otherData": {
            "build": summary.build_name,
            "critical_path_ms": summary.critical_path.as_ref().map(|path| path.duration_ms),
        },
    })
}

/// The operation's domain: `artifact` for `artifact:app`.
fn category(operation_id: &str) -> &str {
    operation_id
        .split_once(':')
        .map_or(operation_id, |(domain, _)| domain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::StepTimingRecord;
    use gaia_plan::{OperationKind, PlannedOperation};

    fn timing(id: &str, start_ms: u64, duration_ms: u64, status: &str) -> OperationTimingRecord {
        OperationTimingRecord {
            operation_id: id.into(),
            start_ms,
            duration_ms,
            paused_ms: 0,
            status: status.into(),
            steps: Vec::new(),
        }
    }

    fn plan(edges: &[(&str, &[&str])]) -> ExecutionPlan {
        ExecutionPlan {
            build_id: gaia_spec::BuildId::new("profile"),
            operations: edges
                .iter()
                .map(|(id, deps)| {
                    deps.iter().fold(
                        PlannedOperation::new(OperationId::new(*id), OperationKind::ResolveBuild),
                        |operation, dep| operation.with_dependency(OperationId::new(*dep)),
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn critical_path_follows_the_dependency_that_finished_last() {
        let plan = plan(&[
            ("source:a", &[]),
            ("source:b", &[]),
            ("artifact:app", &["source:a", "source:b"]),
            ("artifact:lib", &["source:a"]),
            ("image:build", &["artifact:app", "artifact:lib"]),
            ("report:emit", &["image:build"]),
        ]);
        let timings = [
            timing("source:a", 0, 1_000, "built"),
            timing("source:b", 0, 5_000, "built"),
            timing("artifact:lib", 1_000, 2_000, "built"),
            timing("artifact:app", 5_000, 60_000, "built"),
            timing("image:build", 65_000, 30_000, "built"),
            timing("report:emit", 95_000, 0, "reused"),
        ];
        let path = critical_path(&plan, &timings).expect("path");
        let ids = path
            .operations
            .iter()
            .map(|entry| entry.operation_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["source:b", "artifact:app", "image:build"]);
        assert_eq!(path.duration_ms, 95_000);
    }

    #[test]
    fn critical_path_passes_through_reused_operations() {
        let plan = plan(&[
            ("source:a", &[]),
            ("artifact:app", &["source:a"]),
            ("image:build", &["artifact:app"]),
        ]);
        let timings = [
            timing("source:a", 0, 3_000, "built"),
            timing("artifact:app", 3_000, 0, "reused"),
            timing("image:build", 3_000, 10_000, "built"),
        ];
        let path = critical_path(&plan, &timings).expect("path");
        assert_eq!(path.operations.len(), 2);
        assert_eq!(path.operations[0].operation_id, "source:a");
        assert_eq!(path.duration_ms, 13_000);
        assert_eq!(
            critical_path(&plan, &[timing("source:a", 0, 0, "reused")]),
            None
        );
    }

    #[test]
    fn trace_puts_overlapping_operations_on_separate_rows_and_nests_assembly_steps() {
        let mut assembly = timing("image:assembly", 10_000, 5_000, "built");
        assembly.steps = vec![
            StepTimingRecord {
                step: "assembly filesystem rootfs".into(),
                duration_ms: 3_000,
            },
            StepTimingRecord {
                step: "assembly disk sd".into(),
                duration_ms: 4_000,
            },
            StepTimingRecord {
                step: "buildroot make".into(),
                duration_ms: 1,
            },
        ];
        let timings = vec![
            timing("artifact:a", 0, 10_000, "built"),
            timing("artifact:b", 2_000, 3_000, "built"),
            assembly,
        ];
        let plan = plan(&[
            ("artifact:a", &[]),
            ("artifact:b", &[]),
            ("image:assembly", &["artifact:a"]),
        ]);
        let summary = RunSummary {
            critical_path: critical_path(&plan, &timings),
            operation_timings: timings,
            build_name: "demo".into(),
            ..RunSummary::default()
        };
        let trace = chrome_trace(&summary);
        let events = trace["traceEvents"].as_array().expect("events");
        let slice = |name: &str| {
            events
                .iter()
                .find(|event| event["name"] == name)
                .unwrap_or_else(|| panic!("no event {name}"))
        };
        assert_eq!(slice("artifact:a")["tid"], 1);
        assert_eq!(slice("artifact:b")["tid"], 2);
        assert_eq!(slice("image:assembly")["tid"], 1);
        assert_eq!(slice("image:assembly")["ts"], 10_000_000);
        assert_eq!(slice("image:assembly")["dur"], 5_000_000);
        assert_eq!(slice("image:assembly")["args"]["critical_path"], true);
        assert_eq!(slice("artifact:b")["args"]["critical_path"], false);
        assert_eq!(slice("image:assembly")["cat"], "image");
        // Steps nest inside the operation, clamped to its end.
        assert_eq!(slice("filesystem rootfs")["ts"], 10_000_000);
        assert_eq!(slice("disk sd")["ts"], 13_000_000);
        assert_eq!(slice("disk sd")["dur"], 2_000_000);
        assert_eq!(
            slice("image:assembly")["args"]["steps"][0]["step"],
            "buildroot make"
        );
        assert_eq!(trace["otherData"]["critical_path_ms"], 15_000);
        assert_eq!(trace["displayTimeUnit"], "ms");
    }
}
