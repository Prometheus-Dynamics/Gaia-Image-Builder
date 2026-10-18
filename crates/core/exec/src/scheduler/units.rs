//! Scheduling units (one operation, or a batch of artifact builds that share
//! one provider invocation) and the CPU budget of CPU-heavy units.

use std::borrow::Cow;
use std::collections::HashSet;
use std::thread;

use gaia_plan::{OperationKind, PlannedOperation};

use super::{
    OperationResources, ParallelResourceKey, RunningLedger, ScheduleReadyContext, early_cutoff,
    supports_parallel_runtime,
};
use crate::operations::artifact_batch_key;

/// Operations whose spawned tools use every core by default.
pub(crate) fn is_cpu_heavy(operation: &PlannedOperation) -> bool {
    operation.reuse.should_execute()
        && matches!(
            operation.kind,
            OperationKind::BuildArtifact { .. }
                | OperationKind::PrepareImage
                | OperationKind::BuildImage
        )
}

/// Ready artifact builds that can join `leader`'s provider invocation: same
/// batch key, dependencies finished, and no output collision with running
/// work or with each other.
pub(crate) fn batch_companions<'env>(
    context: &ScheduleReadyContext<'env>,
    leader_index: usize,
    leader: &PlannedOperation,
    remaining_dependencies: &[usize],
    completed: &[bool],
    running: &[bool],
    ledger: &RunningLedger,
) -> Vec<(usize, PlannedOperation)> {
    let spec = context.spec;
    let plan = context.plan;
    let resources = context.resources;
    if !supports_parallel_runtime(leader.parallelism.mode.clone(), &leader.parallelism.domain) {
        return Vec::new();
    }
    let Some(key) = artifact_batch_key(leader, spec, context.providers) else {
        return Vec::new();
    };
    // Build inputs are not claimed: companions of one batch share them, and
    // only outputs collide. Output keys never include build inputs, so
    // checking every held key is the same as checking the outputs alone.
    let mut claimed: HashSet<ParallelResourceKey> =
        output_keys(resources, leader_index).into_iter().collect();
    let mut companions = Vec::new();
    for (index, candidate) in plan.operations.iter().enumerate() {
        if index == leader_index
            || running[index]
            || completed[index]
            || remaining_dependencies[index] != 0
            || !matches!(candidate.kind, OperationKind::BuildArtifact { .. })
            || !supports_parallel_runtime(
                candidate.parallelism.mode.clone(),
                &candidate.parallelism.domain,
            )
        {
            continue;
        }
        let candidate = early_cutoff(spec, plan, candidate);
        if artifact_batch_key(&candidate, spec, context.providers).as_deref() != Some(&key) {
            continue;
        }
        let outputs = output_keys(resources, index);
        if outputs
            .iter()
            .any(|resource| claimed.contains(resource) || ledger.holds(resource))
        {
            continue;
        }
        claimed.extend(outputs);
        companions.push((index, Cow::into_owned(candidate)));
    }
    companions
}

fn output_keys(resources: &OperationResources, index: usize) -> Vec<ParallelResourceKey> {
    resources
        .keys(index)
        .iter()
        .filter(|resource| !matches!(resource, ParallelResourceKey::ArtifactBuildInput { .. }))
        .cloned()
        .collect()
}

pub(crate) struct BudgetInputs<'a> {
    pub(crate) remaining_dependencies: &'a [usize],
    pub(crate) completed: &'a [bool],
    pub(crate) running: &'a [bool],
    pub(crate) ledger: &'a RunningLedger,
    pub(crate) running_units: usize,
}

/// Job budget for a CPU-heavy unit about to start (see
/// [`budget_from_free_cores`]). The heavy units that may start beside it
/// are the ready ones with free job slots and no resource conflicts.
pub(crate) fn unit_job_budget(
    context: &ScheduleReadyContext<'_>,
    unit: &[(usize, PlannedOperation)],
    inputs: BudgetInputs<'_>,
) -> Option<usize> {
    let plan = context.plan;
    let resources = context.resources;
    let in_unit = unit.iter().map(|(index, _)| *index).collect::<HashSet<_>>();
    let mut claimed = unit
        .iter()
        .flat_map(|(index, _)| resources.keys(*index).iter().cloned())
        .collect::<HashSet<_>>();
    let free_slots = context
        .max_parallel_jobs
        .saturating_sub(inputs.running_units + 1);
    let mut waiting_heavy = 0usize;
    for (index, candidate) in plan.operations.iter().enumerate() {
        if waiting_heavy >= free_slots {
            break;
        }
        if in_unit.contains(&index)
            || inputs.running[index]
            || inputs.completed[index]
            || inputs.remaining_dependencies[index] != 0
            || !is_cpu_heavy(candidate)
            || !supports_parallel_runtime(
                candidate.parallelism.mode.clone(),
                &candidate.parallelism.domain,
            )
        {
            continue;
        }
        let keys = resources.keys(index);
        if keys
            .iter()
            .any(|resource| claimed.contains(resource) || inputs.ledger.holds(resource))
        {
            continue;
        }
        claimed.extend(keys.iter().cloned());
        waiting_heavy += 1;
    }
    budget_from_free_cores(
        available_cores(),
        inputs.ledger.held_cores(),
        1 + waiting_heavy,
    )
}

pub(crate) fn available_cores() -> usize {
    thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .max(1)
}

/// Job budget for a unit that is about to start: the cores the running heavy
/// units do not hold, split across this unit and the heavy units expected to
/// run beside it. Tools read their job count once, when they start, so a
/// running unit keeps its budget; a unit that finishes returns its cores,
/// and the units that start after it are sized from the larger free pool.
/// `None` when the unit runs alone with no core held, so its tools keep their
/// defaults.
pub(crate) fn budget_from_free_cores(
    cores: usize,
    held_cores: usize,
    concurrent: usize,
) -> Option<usize> {
    if concurrent <= 1 && held_cores == 0 {
        return None;
    }
    let free = cores.saturating_sub(held_cores);
    Some((free / concurrent.max(1)).max(1))
}

#[cfg(test)]
mod tests {
    use super::budget_from_free_cores;

    #[test]
    fn a_unit_alone_on_an_idle_machine_keeps_the_tool_defaults() {
        assert_eq!(budget_from_free_cores(8, 0, 1), None);
    }

    #[test]
    fn idle_cores_are_split_across_the_units_that_start_together() {
        assert_eq!(budget_from_free_cores(8, 0, 2), Some(4));
        assert_eq!(budget_from_free_cores(8, 0, 3), Some(2));
    }

    #[test]
    fn a_new_unit_is_sized_from_the_cores_running_units_do_not_hold() {
        // One running unit holds 4 of 8 cores: the new one gets the rest.
        assert_eq!(budget_from_free_cores(8, 4, 1), Some(4));
        // Two running units hold 6: what is left is shared by the new unit
        // and one that may start beside it.
        assert_eq!(budget_from_free_cores(8, 6, 2), Some(1));
    }

    #[test]
    fn a_unit_always_gets_at_least_one_job() {
        assert_eq!(budget_from_free_cores(8, 8, 1), Some(1));
        assert_eq!(budget_from_free_cores(2, 5, 4), Some(1));
    }
}
