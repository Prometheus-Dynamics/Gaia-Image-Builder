use gaia_exec::{ExecutionEvent, ExecutionOutcome};
use gaia_plan::{ExecutionPlan, OperationReuse};
use gaia_spec::ResolvedBuildSpec;

use crate::model::RebuildReasonReport;

pub fn render_rebuild_reasons(
    spec: &ResolvedBuildSpec,
    plan: &ExecutionPlan,
    outcome: &ExecutionOutcome,
) -> Vec<RebuildReasonReport> {
    let mut reasons = plan
        .operations
        .iter()
        .flat_map(|operation| {
            let mut entries = Vec::new();
            match &operation.reuse {
                OperationReuse::Execute(reason) => entries.push(RebuildReasonReport {
                    operation_id: operation.id.as_str().to_string(),
                    code: reason.code,
                    message: reason.message.clone(),
                }),
                OperationReuse::Reuse { .. } => {}
            }
            if operation.optionality != gaia_plan::OperationOptionality::Required {
                entries.push(RebuildReasonReport {
                    operation_id: operation.id.as_str().to_string(),
                    code: "operation_optionality",
                    message: format!(
                        "operation '{}' is marked {}",
                        operation.id.as_str(),
                        operation.optionality.as_str()
                    ),
                });
            }
            entries
        })
        .collect::<Vec<_>>();

    for event in &outcome.events {
        if let Some(reason) = rebuild_reason_from_event(event) {
            reasons.push(reason);
        }
    }

    if !spec.policy.failure.rollback_on_error && !outcome.errors.is_empty() {
        reasons.push(RebuildReasonReport {
            operation_id: "run:failure-policy".to_string(),
            code: "rollback_disabled",
            message: "rollback skipped because rollback_on_error=false".to_string(),
        });
    }

    reasons
}

fn rebuild_reason_from_event(event: &ExecutionEvent) -> Option<RebuildReasonReport> {
    let ExecutionEvent::Log {
        operation_id,
        message,
    } = event
    else {
        return None;
    };

    let code = if message.starts_with("rolled back ") {
        "rollback_performed"
    } else if message.starts_with("cleaned ")
        && (message.contains("partial output") || message.contains("output path"))
    {
        "failed_outputs_cleaned"
    } else if message.starts_with("preserved ") && message.contains("failed output path") {
        "failed_outputs_preserved"
    } else if message.contains("rollback domain is disabled") {
        "rollback_domain_disabled"
    } else {
        return None;
    };

    Some(RebuildReasonReport {
        operation_id: operation_id.as_str().to_string(),
        code,
        message: message.clone(),
    })
}
