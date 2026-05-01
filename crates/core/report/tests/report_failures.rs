pub mod support;

use gaia_exec::{
    ExecutionCleanupStatus, ExecutionError, ExecutionErrorKind, ExecutionEvent, ExecutionOutcome,
};
use gaia_plan::{OperationId, plan_build};
use gaia_report::generate_report;
use gaia_spec::RollbackDomain;
use gaia_validate::validate_spec_with_providers;
use support::{provider_catalogs, test_spec};

fn execution_error(
    code: &'static str,
    kind: ExecutionErrorKind,
    operation_id: OperationId,
    message: &str,
    output_tail: Vec<String>,
) -> ExecutionError {
    ExecutionError {
        code,
        kind,
        operation_id,
        message: message.into(),
        output_tail,
        cleanup_domain: None,
        cleanup_paths: Vec::new(),
        cleanup_status: ExecutionCleanupStatus::NotRequired,
        cleanup_failures: Vec::new(),
    }
}

#[test]
fn rebuild_reasons_include_rollback_and_disabled_policy_entries() {
    let mut spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let validation =
        validate_spec_with_providers(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let rollback_outcome = ExecutionOutcome {
        rolled_back_ids: vec![OperationId::source(&spec.sources[0].id)],
        errors: vec![execution_error(
            "source_execution_failed",
            ExecutionErrorKind::OutputMissing,
            OperationId::source(&spec.sources[0].id),
            "source failed",
            Vec::new(),
        )],
        events: vec![
            ExecutionEvent::Log {
                operation_id: OperationId::source(&spec.sources[0].id),
                message: "cleaned 1 partial output(s)".into(),
            },
            ExecutionEvent::Log {
                operation_id: OperationId::artifact(&spec.artifacts[0].id),
                message: "rolled back 2 output path(s)".into(),
            },
        ],
        ..ExecutionOutcome::default()
    };

    let rollback_report = generate_report(&spec, &validation, &plan, &rollback_outcome);
    assert!(
        rollback_report
            .rebuild_reasons
            .iter()
            .any(|reason| reason.code == "failed_outputs_cleaned")
    );
    assert!(
        rollback_report
            .rebuild_reasons
            .iter()
            .any(|reason| reason.code == "rollback_performed")
    );

    spec.policy.failure.rollback_on_error = false;
    let disabled_outcome = ExecutionOutcome {
        errors: vec![execution_error(
            "artifact_execution_failed",
            ExecutionErrorKind::BackendCommand,
            OperationId::artifact(&spec.artifacts[0].id),
            "artifact failed",
            Vec::new(),
        )],
        ..ExecutionOutcome::default()
    };

    let disabled_report = generate_report(&spec, &validation, &plan, &disabled_outcome);
    assert!(
        disabled_report
            .rebuild_reasons
            .iter()
            .any(|reason| reason.code == "rollback_disabled")
    );
}

#[test]
fn typed_execution_kinds_flow_into_failure_classes_without_heuristics() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let validation =
        validate_spec_with_providers(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let outcome = ExecutionOutcome {
        errors: vec![
            execution_error(
                "missing_image_provider",
                ExecutionErrorKind::MissingProvider,
                OperationId::image(),
                "missing image provider",
                Vec::new(),
            ),
            execution_error(
                "stage_runtime_state_failed",
                ExecutionErrorKind::RuntimeState,
                OperationId::stage_file(&spec.stage.files[0].id),
                "failed to write runtime state",
                Vec::new(),
            ),
        ],
        ..ExecutionOutcome::default()
    };

    let report = generate_report(&spec, &validation, &plan, &outcome);

    assert!(
        report
            .execution_failures
            .iter()
            .any(|failure| failure.class == gaia_report::FailureClass::MissingProvider)
    );
    assert!(
        report
            .execution_failures
            .iter()
            .any(|failure| failure.class == gaia_report::FailureClass::RuntimeState)
    );
    assert!(
        report
            .summary
            .failure_classes
            .iter()
            .any(
                |entry| entry.class == gaia_report::FailureClass::MissingProvider
                    && entry.count == 1
            )
    );
    assert!(
        report
            .summary
            .failure_classes
            .iter()
            .any(
                |entry| entry.class == gaia_report::FailureClass::RuntimeState && entry.count == 1
            )
    );
}

#[test]
fn failed_execution_report_preserves_output_tail() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let validation =
        validate_spec_with_providers(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let output_tail = vec![
        "build stdout: compiling crate".to_string(),
        "build stderr: linker failed".to_string(),
    ];
    let outcome = ExecutionOutcome {
        errors: vec![execution_error(
            "artifact_execution_failed",
            ExecutionErrorKind::BackendCommand,
            OperationId::artifact(&spec.artifacts[0].id),
            "artifact failed",
            output_tail.clone(),
        )],
        ..ExecutionOutcome::default()
    };

    let report = generate_report(&spec, &validation, &plan, &outcome);

    assert_eq!(report.execution_failures[0].output_tail, output_tail);
}

#[test]
fn failed_execution_report_includes_cleanup_status_and_failures() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let validation =
        validate_spec_with_providers(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let cleanup_path = std::path::PathBuf::from("/tmp/gaia-cleanup-output");
    let mut error = execution_error(
        "artifact_execution_failed",
        ExecutionErrorKind::BackendCommand,
        OperationId::artifact(&spec.artifacts[0].id),
        "artifact failed",
        Vec::new(),
    );
    error.cleanup_domain = Some(RollbackDomain::Artifacts);
    error.cleanup_paths = vec![cleanup_path.clone()];
    error.cleanup_status = ExecutionCleanupStatus::Failed;
    error.cleanup_failures = vec!["failed to remove cleanup path".into()];

    let outcome = ExecutionOutcome {
        errors: vec![error],
        cleanup_failures: vec![gaia_exec::CleanupFailure {
            operation_id: OperationId::artifact(&spec.artifacts[0].id),
            path: cleanup_path.clone(),
            message: "failed to remove cleanup path".into(),
        }],
        ..ExecutionOutcome::default()
    };

    let report = generate_report(&spec, &validation, &plan, &outcome);

    assert_eq!(report.summary.cleanup_failure_count, 1);
    assert_eq!(
        report.execution_failures[0].cleanup_status,
        gaia_report::CleanupStatus::Failed
    );
    assert_eq!(
        report.execution_failures[0].cleanup_domain.as_deref(),
        Some("artifacts")
    );
    assert_eq!(
        report.execution_failures[0].cleanup_paths,
        vec![cleanup_path.display().to_string()]
    );
    assert_eq!(
        report.execution_failures[0].cleanup_failures,
        vec!["failed to remove cleanup path".to_string()]
    );
}
