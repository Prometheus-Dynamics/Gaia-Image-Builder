//! The reasons a plan gives for executing an operation, explained from the
//! inputs the last run recorded. `gaia run` and `gaia preview` both use it, so
//! the run log, `rebuild-reasons.json` and the preview name the same changed
//! inputs.

use gaia_plan::{
    ExecutionPlan, OperationReuse, PlannedOperation, ReuseState, fingerprint_change_detail,
};
use gaia_spec::ResolvedBuildSpec;
use std::collections::BTreeMap;

use super::state::{RecordedComponents, load_operation_components};

/// Message of a fingerprint change with no recorded input detail (state
/// written before per-input records existed, or records that do not match).
const NO_DETAIL: &str = "fingerprint changed (no detail recorded)";

/// The inputs recorded by the last run, read from the details sidecar. Must be
/// read before a run saves its own state, which overwrites the sidecar. Empty
/// when there is no reuse state, since then nothing can be explained.
pub fn recorded_for_explanation(
    spec: &ResolvedBuildSpec,
    reuse_state: Option<&ReuseState>,
) -> BTreeMap<String, RecordedComponents> {
    if reuse_state.is_some() {
        load_operation_components(spec)
    } else {
        BTreeMap::new()
    }
}

/// Replaces the generic "fingerprint changed (no detail recorded)" reason of
/// each executing operation with the named inputs that differ from the
/// recorded ones, such as `config_overrides changed (BR2_A)`. The plan is
/// explained whole, before any target restriction, because an operation's
/// dependencies are part of its inputs.
pub fn explain_rebuild_reasons(
    spec: &ResolvedBuildSpec,
    plan: &mut ExecutionPlan,
    reuse_state: Option<&ReuseState>,
    recorded: &BTreeMap<String, RecordedComponents>,
) {
    let Some(state) = reuse_state else {
        return;
    };
    let needs_detail = |operation: &PlannedOperation| {
        matches!(
            &operation.reuse,
            OperationReuse::Execute(reason) if reason.message.contains(NO_DETAIL)
        )
    };
    if !plan.operations.iter().any(needs_detail) {
        return;
    }
    // The inputs are read from the plan as it was, dependencies included.
    let snapshot = plan.clone();
    for operation in &mut plan.operations {
        if !needs_detail(&*operation) {
            continue;
        }
        let id = operation.id.as_str().to_string();
        // A record of another fingerprint describes an older state: ignored.
        let Some(record) = recorded.get(&id) else {
            continue;
        };
        if state.operation_fingerprints.get(&id) != Some(&record.fingerprint) {
            continue;
        }
        let current = gaia_plan::operation_components(spec, &snapshot, operation);
        let message =
            fingerprint_change_detail(&id, &record.components, &current).unwrap_or_else(|| {
                format!(
                    "operation '{id}' will execute because its fingerprint changed, \
                     yet every recorded input matches"
                )
            });
        if let OperationReuse::Execute(reason) = &mut operation.reuse {
            reason.message = message;
        }
    }
}
