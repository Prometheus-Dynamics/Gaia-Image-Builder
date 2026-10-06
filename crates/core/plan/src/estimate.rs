//! Duration estimates for a plan from previously recorded operation timings.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use crate::{ExecutionPlan, OperationId};

/// Estimated cost of the operations a plan will execute.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanEstimate {
    /// Sum of the recorded durations of every operation that will execute
    /// (the time a strictly serial run would take).
    pub total_work: Duration,
    /// Executing operations on the dependency chain with the largest summed
    /// duration, in dependency order. A lower bound on wall-clock time,
    /// whatever the parallelism.
    pub critical_path: Vec<OperationId>,
    pub critical_path_duration: Duration,
    /// Executing operations with a recorded duration.
    pub timed_operations: usize,
    /// Executing operations with no recorded duration (counted as zero).
    pub untimed_operations: Vec<OperationId>,
}

impl PlanEstimate {
    pub fn executing_operations(&self) -> usize {
        self.timed_operations + self.untimed_operations.len()
    }
}

/// Estimates `plan` from `durations_ms`, the last recorded wall-clock time
/// per operation id. Reused operations cost nothing.
pub fn estimate_plan(plan: &ExecutionPlan, durations_ms: &BTreeMap<String, u64>) -> PlanEstimate {
    let index = plan
        .operations
        .iter()
        .enumerate()
        .map(|(position, operation)| (operation.id.as_str(), position))
        .collect::<HashMap<_, _>>();
    let mut estimate = PlanEstimate::default();
    let cost = plan
        .operations
        .iter()
        .map(|operation| {
            if !operation.reuse.should_execute() {
                return 0;
            }
            match durations_ms.get(operation.id.as_str()) {
                Some(ms) => {
                    estimate.timed_operations += 1;
                    *ms
                }
                None => {
                    estimate.untimed_operations.push(operation.id.clone());
                    0
                }
            }
        })
        .collect::<Vec<_>>();
    estimate.total_work = Duration::from_millis(cost.iter().sum());

    // Longest path ending at each operation, memoized; an explicit stack
    // keeps deep plans off the call stack, and `visiting` breaks cycles.
    let count = plan.operations.len();
    let mut best: Vec<Option<(u64, Option<usize>)>> = vec![None; count];
    let mut visiting = vec![false; count];
    for root in 0..count {
        let mut stack = vec![root];
        while let Some(&current) = stack.last() {
            if best[current].is_some() {
                stack.pop();
                continue;
            }
            visiting[current] = true;
            let dependencies = plan.operations[current]
                .depends_on
                .iter()
                .filter_map(|dependency| index.get(dependency.as_str()).copied())
                .collect::<Vec<_>>();
            let pending = dependencies
                .iter()
                .copied()
                .find(|&dependency| best[dependency].is_none() && !visiting[dependency]);
            if let Some(dependency) = pending {
                stack.push(dependency);
                continue;
            }
            let heaviest = dependencies
                .iter()
                .filter_map(|&dependency| best[dependency].map(|(total, _)| (total, dependency)))
                .max_by_key(|(total, _)| *total);
            best[current] = Some(match heaviest {
                Some((total, dependency)) => (total + cost[current], Some(dependency)),
                None => (cost[current], None),
            });
            visiting[current] = false;
            stack.pop();
        }
    }
    let end = (0..count).max_by_key(|&position| best[position].map_or(0, |(total, _)| total));
    if let Some(end) = end {
        estimate.critical_path_duration = Duration::from_millis(best[end].map_or(0, |(t, _)| t));
        let mut path = Vec::new();
        let mut cursor = Some(end);
        while let Some(position) = cursor {
            if cost[position] > 0 {
                path.push(plan.operations[position].id.clone());
            }
            cursor = best[position].and_then(|(_, previous)| previous);
        }
        path.reverse();
        estimate.critical_path = path;
    }
    estimate
}

/// `1h02m`, `3m05s`, `12s` or `850ms`.
pub fn format_duration_short(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds == 0 {
        return format!("{}ms", duration.as_millis());
    }
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;
    if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m{secs:02}s")
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OperationKind, OperationReuse, PlannedOperation};

    fn op(id: &str, deps: &[&str]) -> PlannedOperation {
        let mut operation =
            PlannedOperation::new(OperationId::new(id), OperationKind::ResolveBuild);
        for dependency in deps {
            operation = operation.with_dependency(OperationId::new(*dependency));
        }
        operation
    }

    fn plan(operations: Vec<PlannedOperation>) -> ExecutionPlan {
        ExecutionPlan {
            build_id: gaia_spec::BuildId::new("estimate"),
            operations,
        }
    }

    #[test]
    fn critical_path_follows_heaviest_dependency_chain() {
        // resolve -> {src (10) -> art-a (100), art-b (30)} -> image (500)
        let plan = plan(vec![
            op("resolve", &[]),
            op("src", &["resolve"]),
            op("art-a", &["src"]),
            op("art-b", &["resolve"]),
            op("image", &["art-a", "art-b"]),
        ]);
        let durations = [("src", 10), ("art-a", 100), ("art-b", 30), ("image", 500)]
            .into_iter()
            .map(|(id, ms)| (id.to_string(), ms))
            .collect();

        let estimate = estimate_plan(&plan, &durations);

        assert_eq!(estimate.total_work, Duration::from_millis(640));
        assert_eq!(estimate.critical_path_duration, Duration::from_millis(610));
        assert_eq!(
            estimate
                .critical_path
                .iter()
                .map(OperationId::as_str)
                .collect::<Vec<_>>(),
            ["src", "art-a", "image"]
        );
        assert_eq!(estimate.timed_operations, 4);
        assert_eq!(
            estimate.untimed_operations,
            vec![OperationId::new("resolve")]
        );
    }

    #[test]
    fn reused_operations_cost_nothing() {
        let plan = plan(vec![
            op("slow", &[]).with_reuse(OperationReuse::Reuse {
                source: "state".into(),
            }),
            op("fast", &["slow"]),
        ]);
        let durations = [("slow", 1_000), ("fast", 5)]
            .into_iter()
            .map(|(id, ms)| (id.to_string(), ms))
            .collect();

        let estimate = estimate_plan(&plan, &durations);

        assert_eq!(estimate.total_work, Duration::from_millis(5));
        assert_eq!(estimate.critical_path, vec![OperationId::new("fast")]);
        assert_eq!(estimate.executing_operations(), 1);
    }

    #[test]
    fn rust_feature_flags_change_the_artifact_fingerprint() {
        let mut spec = gaia_spec::ResolvedBuildSpec::new("features-fingerprint");
        spec.artifacts.push(gaia_spec::ArtifactSpec::new(
            "node",
            gaia_spec::ArtifactDefinition::Rust(gaia_spec::RustArtifactSpec {
                package: "orion-node".into(),
                target_name: None,
                variant: gaia_spec::ArtifactVariantSpec::File,
                features: Vec::new(),
                no_default_features: false,
                all_features: false,
                build_group: None,
                group_packages: Vec::new(),
            }),
            None,
            gaia_spec::ArtifactOutputSpec {
                path: "out/node".into(),
            },
        ));
        let kind = OperationKind::BuildArtifact {
            artifact_id: gaia_spec::ArtifactId::new("node"),
        };
        let fingerprint =
            |spec: &gaia_spec::ResolvedBuildSpec| crate::operation_fingerprint(spec, &kind);
        let baseline = fingerprint(&spec);
        let mut changes = Vec::new();
        for change in 0..3 {
            let mut changed = spec.clone();
            let gaia_spec::ArtifactDefinition::Rust(rust) = &mut changed.artifacts[0].definition
            else {
                unreachable!()
            };
            match change {
                0 => rust.features = vec!["tls".into()],
                1 => rust.no_default_features = true,
                _ => rust.all_features = true,
            }
            changes.push(fingerprint(&changed));
        }
        assert!(changes.iter().all(|changed| *changed != baseline));
        assert_eq!(fingerprint(&spec.clone()), baseline);
    }

    #[test]
    fn formats_short_durations() {
        assert_eq!(format_duration_short(Duration::from_millis(850)), "850ms");
        assert_eq!(format_duration_short(Duration::from_secs(12)), "12s");
        assert_eq!(format_duration_short(Duration::from_secs(185)), "3m05s");
        assert_eq!(format_duration_short(Duration::from_secs(3720)), "1h02m");
    }
}
