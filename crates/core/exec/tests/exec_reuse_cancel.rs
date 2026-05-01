pub mod support;

use gaia_exec::{
    ExecutionCancellation, ExecutionEvent, ExecutionProviders, execute_plan,
    execute_plan_with_cancellation,
};
use gaia_plan::{plan_build, plan_build_with_reuse_state};
use support::{materialize_reusable_outputs, provider_catalogs, reuse_state_for_ids, test_spec};

#[test]
fn cancelled_run_records_cancellation_and_skips_future_operations() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let cancellation = ExecutionCancellation::new();
    cancellation.cancel();

    let outcome = execute_plan_with_cancellation(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
        &cancellation,
    );

    assert!(outcome.cancelled);
    assert_eq!(
        outcome
            .cancelled_operation_id
            .as_ref()
            .map(|id| id.as_str()),
        Some("resolve-build")
    );
    assert_eq!(outcome.completed_operations, 0);
    assert!(outcome.errors.is_empty());
    assert!(
        outcome
            .events
            .iter()
            .any(|event| matches!(event, ExecutionEvent::Cancelled { .. }))
    );
}

#[test]
fn executes_reused_plan_and_records_reused_operations() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    materialize_reusable_outputs(&spec);
    let baseline_plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let reused_ids = [
        "source:gaia-upstream",
        "source:workspace-root",
        "artifact:gaia-app",
        "install:install-gaia-app",
        "stage:file:motd",
        "stage:env:runtime-env",
        "stage:service:gaia-service",
        "image:build",
        "checkpoint:base-image",
    ];
    let reuse_state = reuse_state_for_ids(&spec, &baseline_plan, &reused_ids);
    let plan = plan_build_with_reuse_state(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(&reuse_state),
    );

    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );

    assert_eq!(outcome.completed_operations, 11);
    assert_eq!(outcome.reused_ids.len(), 9);
    assert!(
        outcome
            .events
            .iter()
            .any(|event| matches!(event, ExecutionEvent::Reused { .. }))
    );
}
