pub mod support;

use gaia_app::{AppArgs, CommandOutcome, run_with_args};
use std::fs;
use std::path::PathBuf;
use support::{config_path, seed_default_assets, unique_dir};

#[test]
fn run_command_surfaces_execution_failures_from_backend_errors() {
    let missing_root_dir = unique_dir("gaia-cli-missing-root");
    let run_out_dir = unique_dir("gaia-cli-fail-out");
    let run_build_dir = unique_dir("gaia-cli-fail-build");
    fs::create_dir_all(&missing_root_dir).expect("workspace root");
    seed_default_assets(&missing_root_dir);

    let run = run_with_args(AppArgs::parse_from(vec![
        "run".to_string(),
        config_path(),
        "--preset".to_string(),
        "ci".to_string(),
        "--set".to_string(),
        format!("workspace.root_dir={missing_root_dir}"),
        "--set".to_string(),
        format!("workspace.out_dir={run_out_dir}"),
        "--set".to_string(),
        format!("workspace.build_dir={run_build_dir}"),
    ]));

    assert_eq!(run.exit_code(), 4);

    match run {
        CommandOutcome::Ran {
            report,
            validation,
            plan_diagnostics,
            execution_errors,
            ..
        } => {
            assert!(validation.errors.is_empty());
            assert!(plan_diagnostics.is_empty());
            assert!(!execution_errors.is_empty(), "expected execution errors");
            assert!(report.summary.error_count > 0);
            assert!(report.summary.rolled_back_operations > 0);
            assert!(report.summary.rollback_on_error);
            assert!(!report.summary.preserve_failed_outputs);
            assert!(!report.summary.failure_classes.is_empty());
            assert!(!report.execution_failures.is_empty());
            assert!(
                report
                    .rebuild_reasons
                    .iter()
                    .any(|reason| reason.code == "rollback_performed")
            );
            assert!(
                report
                    .rebuild_reasons
                    .iter()
                    .any(|reason| reason.code == "failed_outputs_cleaned")
            );
            assert!(execution_errors.iter().any(|error| {
                error.code == "source_execution_failed"
                    || error.code == "artifact_execution_failed"
                    || error.code == "image_execution_failed"
            }));
            assert!(report.summary.failure_classes.iter().any(|entry| format!(
                "{:?}",
                entry.class
            ) == "BackendCommand"
                || format!("{:?}", entry.class) == "ToolStart"
                || format!("{:?}", entry.class) == "OutputMissing"));
        }
        outcome => panic!("expected ran outcome, got {outcome:?}"),
    }
}

#[test]
fn run_command_reports_when_rollback_is_disabled_by_policy() {
    let missing_root_dir = unique_dir("gaia-cli-no-rollback-root");
    let run_out_dir = unique_dir("gaia-cli-no-rollback-out");
    let run_build_dir = unique_dir("gaia-cli-no-rollback-build");
    fs::create_dir_all(&missing_root_dir).expect("workspace root");
    seed_default_assets(&missing_root_dir);

    let run = run_with_args(AppArgs::parse_from(vec![
        "run".to_string(),
        config_path(),
        "--preset".to_string(),
        "ci".to_string(),
        "--set".to_string(),
        "policy.failure.rollback_on_error=false".to_string(),
        "--set".to_string(),
        format!("workspace.root_dir={missing_root_dir}"),
        "--set".to_string(),
        format!("workspace.out_dir={run_out_dir}"),
        "--set".to_string(),
        format!("workspace.build_dir={run_build_dir}"),
    ]));

    assert_eq!(run.exit_code(), 4);

    match run {
        CommandOutcome::Ran {
            report,
            execution_errors,
            ..
        } => {
            assert!(!execution_errors.is_empty(), "expected execution errors");
            assert!(report.summary.error_count > 0);
            assert_eq!(report.summary.rolled_back_operations, 0);
            assert!(!report.summary.rollback_on_error);
            assert!(!report.summary.preserve_failed_outputs);
            assert!(
                report
                    .rebuild_reasons
                    .iter()
                    .any(|reason| reason.code == "rollback_disabled")
            );
            assert!(
                !report
                    .rebuild_reasons
                    .iter()
                    .any(|reason| reason.code == "rollback_performed")
            );
            assert!(
                PathBuf::from(&run_build_dir)
                    .join("sources/gaia-upstream")
                    .exists()
            );
        }
        outcome => panic!("expected ran outcome, got {outcome:?}"),
    }
}
