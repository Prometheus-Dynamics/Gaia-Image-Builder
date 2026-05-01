pub mod support;

use gaia_exec::{ExecutionProviders, execute_plan};
use gaia_plan::plan_build;
use std::path::Path;
use support::{
    artifact_failure_spec_with_overrides, failing_spec, failing_spec_with_overrides,
    provider_catalogs,
};

#[test]
fn failed_run_rolls_back_completed_outputs_from_current_run() {
    let spec = failing_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );

    eprintln!(
        "rolled_back={:?} errors={:?}",
        outcome
            .rolled_back_ids
            .iter()
            .map(|id| id.as_str().to_string())
            .collect::<Vec<_>>(),
        outcome
            .errors
            .iter()
            .map(|error| (
                error.operation_id.as_str().to_string(),
                error.message.clone()
            ))
            .collect::<Vec<_>>()
    );
    assert!(!outcome.errors.is_empty());
    assert_eq!(outcome.completed_operations, 0);
    assert!(!outcome.rolled_back_ids.is_empty());
    assert!(
        !Path::new(&spec.workspace.build_dir)
            .join("sources/gaia-upstream")
            .exists()
    );
    assert!(
        !Path::new(&spec.workspace.out_dir)
            .join("artifacts/gaia")
            .exists()
    );
    assert!(
        !Path::new(&spec.workspace.out_dir)
            .join(".gaia/runtime")
            .exists()
    );
}

#[test]
fn preserve_failed_outputs_policy_keeps_failed_operation_outputs() {
    let spec = failing_spec_with_overrides(vec![(
        "policy.failure.preserve_failed_outputs".into(),
        "true".into(),
    )]);
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );

    eprintln!(
        "rolled_back={:?} completed={:?} errors={:?}",
        outcome
            .rolled_back_ids
            .iter()
            .map(|id| id.as_str().to_string())
            .collect::<Vec<_>>(),
        outcome
            .completed_ids
            .iter()
            .map(|id| id.as_str().to_string())
            .collect::<Vec<_>>(),
        outcome
            .errors
            .iter()
            .map(|error| (
                error.operation_id.as_str().to_string(),
                error.message.clone()
            ))
            .collect::<Vec<_>>()
    );
    assert!(!outcome.errors.is_empty());
    assert!(!outcome.rolled_back_ids.is_empty());
    assert!(
        !Path::new(&spec.workspace.build_dir)
            .join("sources/gaia-upstream")
            .exists()
    );
    assert!(
        Path::new(&spec.workspace.build_dir)
            .join("sources/workspace-root")
            .exists()
    );
}

#[test]
fn rollback_domains_policy_keeps_sources_but_rolls_back_artifacts() {
    let spec = artifact_failure_spec_with_overrides(vec![(
        "policy.failure.rollback_domains".into(),
        "artifacts,images,installs,stage,checkpoints".into(),
    )]);
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );

    assert!(!outcome.errors.is_empty());
    assert!(
        outcome
            .rolled_back_ids
            .iter()
            .any(|id| id.as_str() == "artifact:gaia-app"),
        "rolled_back={:?} completed={:?} errors={:?}",
        outcome
            .rolled_back_ids
            .iter()
            .map(|id| id.as_str().to_string())
            .collect::<Vec<_>>(),
        outcome
            .completed_ids
            .iter()
            .map(|id| id.as_str().to_string())
            .collect::<Vec<_>>(),
        outcome
            .errors
            .iter()
            .map(|error| (
                error.operation_id.as_str().to_string(),
                error.message.clone()
            ))
            .collect::<Vec<_>>()
    );
    assert!(
        Path::new(&spec.workspace.build_dir)
            .join("sources/workspace-root")
            .exists()
    );
    assert!(
        Path::new(&spec.workspace.build_dir)
            .join("sources/gaia-upstream")
            .exists()
    );
    assert!(
        !Path::new(&spec.workspace.out_dir)
            .join("artifacts/gaia")
            .exists()
    );
}

#[test]
fn rollback_disabled_policy_keeps_current_run_outputs() {
    let spec = failing_spec_with_overrides(vec![(
        "policy.failure.rollback_on_error".into(),
        "false".into(),
    )]);
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );

    assert!(!outcome.errors.is_empty());
    assert!(outcome.rolled_back_ids.is_empty());
    assert_eq!(outcome.completed_operations, 2);
    assert!(
        Path::new(&spec.workspace.build_dir)
            .join("sources/gaia-upstream")
            .exists()
    );
    assert!(
        Path::new(&spec.workspace.build_dir)
            .join("sources/workspace-root")
            .exists()
    );
}
