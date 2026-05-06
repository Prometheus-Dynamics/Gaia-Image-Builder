use gaia_exec::{ExecutionError, ExecutionOutcome};
use gaia_plan::{ExecutionPlan, OperationReuse};
use gaia_spec::ResolvedBuildSpec;
use gaia_validate::ValidationReport;
use std::collections::BTreeMap;

use crate::model::{
    CleanupStatus, ExecutionFailureReport, FailureClass, FailureClassCount, RunSummary,
};
use crate::state::{output_hygiene_warnings, rollback_domains};

pub fn render_summary(
    spec: &ResolvedBuildSpec,
    validation: &ValidationReport,
    plan: &ExecutionPlan,
    outcome: &ExecutionOutcome,
) -> RunSummary {
    let failure_classes = summarize_failure_classes(&outcome.errors);
    let checkpoint_built_count = plan
        .operations
        .iter()
        .filter(|operation| {
            matches!(
                operation.kind,
                gaia_plan::OperationKind::CaptureCheckpoint { .. }
            ) && matches!(operation.reuse, OperationReuse::Execute(_))
        })
        .count();
    let checkpoint_reused_count = plan
        .operations
        .iter()
        .filter(|operation| {
            matches!(
                operation.kind,
                gaia_plan::OperationKind::CaptureCheckpoint { .. }
            ) && matches!(operation.reuse, OperationReuse::Reuse { .. })
        })
        .count();
    let primary_image_output = outcome
        .image_results
        .iter()
        .find_map(|result| result.archive_path.as_ref())
        .or_else(|| {
            outcome
                .image_results
                .iter()
                .find_map(|result| result.collect_dir.as_ref())
        })
        .map(|path| path.display().to_string());
    let image_reuse_details = outcome
        .image_results
        .iter()
        .flat_map(|result| result.reuse_details.clone())
        .collect::<Vec<_>>();
    RunSummary {
        build_name: spec.identity.display_name.clone(),
        build_version: spec.identity.version.clone(),
        build_description: spec.metadata.description.clone(),
        build_branch: spec.metadata.branch.clone(),
        build_target: spec.metadata.target.clone(),
        build_profile: spec.metadata.profile.clone(),
        primary_image_output,
        operation_count: plan.operations.len(),
        warning_count: validation.warnings.len() + output_hygiene_warnings(spec).len(),
        error_count: validation.errors.len() + outcome.errors.len(),
        completed_operations: outcome.completed_operations,
        reused_operations: outcome.reused_ids.len(),
        image_reused: outcome.image_results.iter().any(|result| result.reused),
        image_reuse_details,
        rolled_back_operations: outcome.rolled_back_ids.len(),
        cleanup_failure_count: outcome.cleanup_failures.len(),
        rollback_on_error: spec.policy.failure.rollback_on_error,
        preserve_failed_outputs: spec.policy.failure.preserve_failed_outputs,
        rollback_domains: rollback_domains(spec),
        source_count: spec.sources.len(),
        artifact_count: spec.artifacts.len(),
        install_count: spec.install.entries.len(),
        stage_file_count: spec.stage.files.len(),
        stage_env_set_count: spec.stage.env_sets.len(),
        stage_service_count: spec.stage.services.len(),
        checkpoint_count: spec.checkpoints.points.len(),
        checkpoint_built_count,
        checkpoint_reused_count,
        failure_classes,
    }
}

pub fn render_execution_failures(outcome: &ExecutionOutcome) -> Vec<ExecutionFailureReport> {
    outcome
        .errors
        .iter()
        .map(|error| ExecutionFailureReport {
            operation_id: error.operation_id.as_str().to_string(),
            code: error.code.to_string(),
            class: classify_execution_error(error),
            message: error.message.clone(),
            output_tail: error.output_tail.clone(),
            cleanup_domain: error
                .cleanup_domain
                .map(|domain| domain.as_str().to_string()),
            cleanup_paths: error
                .cleanup_paths
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            cleanup_status: cleanup_status(error.cleanup_status),
            cleanup_failures: error.cleanup_failures.clone(),
        })
        .collect()
}

fn cleanup_status(status: gaia_exec::ExecutionCleanupStatus) -> CleanupStatus {
    match status {
        gaia_exec::ExecutionCleanupStatus::NotRequired => CleanupStatus::NotRequired,
        gaia_exec::ExecutionCleanupStatus::Cleaned => CleanupStatus::Cleaned,
        gaia_exec::ExecutionCleanupStatus::Preserved => CleanupStatus::Preserved,
        gaia_exec::ExecutionCleanupStatus::DomainDisabled => CleanupStatus::DomainDisabled,
        gaia_exec::ExecutionCleanupStatus::Failed => CleanupStatus::Failed,
    }
}

fn summarize_failure_classes(errors: &[ExecutionError]) -> Vec<FailureClassCount> {
    let mut counts = BTreeMap::<FailureClass, usize>::new();
    for error in errors {
        *counts.entry(classify_execution_error(error)).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(class, count)| FailureClassCount { class, count })
        .collect()
}

fn classify_execution_error(error: &ExecutionError) -> FailureClass {
    match error.kind {
        gaia_exec::ExecutionErrorKind::MissingSpec => FailureClass::MissingSpec,
        gaia_exec::ExecutionErrorKind::MissingProvider => FailureClass::MissingProvider,
        gaia_exec::ExecutionErrorKind::ToolStart => FailureClass::ToolStart,
        gaia_exec::ExecutionErrorKind::Timeout => FailureClass::Timeout,
        gaia_exec::ExecutionErrorKind::Cancelled => FailureClass::Cancelled,
        gaia_exec::ExecutionErrorKind::OutputMissing => FailureClass::OutputMissing,
        gaia_exec::ExecutionErrorKind::BackendCommand => FailureClass::BackendCommand,
        gaia_exec::ExecutionErrorKind::PolicyBlocked => FailureClass::PolicyBlocked,
        gaia_exec::ExecutionErrorKind::RuntimeState => FailureClass::RuntimeState,
        gaia_exec::ExecutionErrorKind::Unknown => match error.code {
            code if code.starts_with("missing_") => {
                if code.ends_with("_provider") {
                    FailureClass::MissingProvider
                } else {
                    FailureClass::MissingSpec
                }
            }
            _ => classify_execution_message(&error.message),
        },
    }
}

fn classify_execution_message(message: &str) -> FailureClass {
    let lowered = message.to_ascii_lowercase();
    if lowered.contains("failed to start ") {
        FailureClass::ToolStart
    } else if lowered.contains("timed out") {
        FailureClass::Timeout
    } else if lowered.contains("was not found")
        || lowered.contains("no tarball was produced")
        || lowered.contains("no wheel file was produced")
        || lowered.contains("built target")
    {
        FailureClass::OutputMissing
    } else if lowered.contains("policy.providers.")
        || lowered.contains("policy-blocked")
        || lowered.contains("skipped because policy")
    {
        FailureClass::PolicyBlocked
    } else if lowered.contains("runtime state")
        || lowered.contains("runtime/")
        || lowered.contains(".state")
    {
        FailureClass::RuntimeState
    } else if lowered.contains("failed") {
        FailureClass::BackendCommand
    } else {
        FailureClass::Unknown
    }
}
