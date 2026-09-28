//! Scheduling units (one operation, or a batch of artifact builds that share
//! one provider invocation) and the CPU budget of CPU-heavy units.

use std::borrow::Cow;
use std::collections::HashSet;
use std::thread;

use gaia_plan::{OperationKind, PlannedOperation};

use super::{
    ScheduleReadyContext, early_cutoff, operation_parallel_resource_keys, supports_parallel_runtime,
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
) -> Vec<(usize, PlannedOperation)> {
    let spec = context.spec;
    let plan = context.plan;
    if !supports_parallel_runtime(leader.parallelism.mode.clone(), &leader.parallelism.domain) {
        return Vec::new();
    }
    let Some(key) = artifact_batch_key(leader, spec, context.providers) else {
        return Vec::new();
    };
    let mut claimed = running
        .iter()
        .enumerate()
        .filter(|(_, is_running)| **is_running)
        .flat_map(|(index, _)| operation_parallel_resource_keys(spec, &plan.operations[index]))
        .filter(|resource| {
            !matches!(
                resource,
                super::ParallelResourceKey::ArtifactBuildInput { .. }
            )
        })
        .collect::<HashSet<_>>();
    claimed.extend(output_keys(context, leader));
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
        let outputs = output_keys(context, &candidate);
        if outputs.iter().any(|resource| claimed.contains(resource)) {
            continue;
        }
        claimed.extend(outputs);
        companions.push((index, Cow::into_owned(candidate)));
    }
    companions
}

fn output_keys(
    context: &ScheduleReadyContext<'_>,
    operation: &PlannedOperation,
) -> Vec<super::ParallelResourceKey> {
    operation_parallel_resource_keys(context.spec, operation)
        .into_iter()
        .filter(|resource| {
            !matches!(
                resource,
                super::ParallelResourceKey::ArtifactBuildInput { .. }
            )
        })
        .collect()
}

pub(crate) struct BudgetInputs<'a> {
    pub(crate) remaining_dependencies: &'a [usize],
    pub(crate) completed: &'a [bool],
    pub(crate) running: &'a [bool],
    pub(crate) running_units: usize,
    pub(crate) running_heavy_units: usize,
}

/// Job budget for a CPU-heavy unit about to start: the available cores
/// split evenly across the heavy units expected to run at the same time
/// (those already running, this one, and ready heavy operations that could
/// start alongside it given free job slots and resource conflicts).
/// `None` when this unit is expected to run alone, so a single operation
/// keeps its tools' defaults.
pub(crate) fn unit_job_budget(
    context: &ScheduleReadyContext<'_>,
    unit: &[(usize, PlannedOperation)],
    inputs: BudgetInputs<'_>,
) -> Option<usize> {
    let spec = context.spec;
    let plan = context.plan;
    let in_unit = unit.iter().map(|(index, _)| *index).collect::<HashSet<_>>();
    let mut claimed = inputs
        .running
        .iter()
        .enumerate()
        .filter(|(_, is_running)| **is_running)
        .flat_map(|(index, _)| operation_parallel_resource_keys(spec, &plan.operations[index]))
        .collect::<HashSet<_>>();
    claimed.extend(
        unit.iter()
            .flat_map(|(_, operation)| operation_parallel_resource_keys(spec, operation)),
    );
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
        let resources = operation_parallel_resource_keys(spec, candidate);
        if resources.iter().any(|resource| claimed.contains(resource)) {
            continue;
        }
        claimed.extend(resources);
        waiting_heavy += 1;
    }
    split_job_budget(
        available_cores(),
        inputs.running_heavy_units + 1 + waiting_heavy,
    )
}

pub(crate) fn available_cores() -> usize {
    thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .max(1)
}

/// Static split of `cores` across `concurrent` heavy units; `None` when
/// only one runs.
pub(crate) fn split_job_budget(cores: usize, concurrent: usize) -> Option<usize> {
    (concurrent > 1).then(|| (cores / concurrent).max(1))
}
