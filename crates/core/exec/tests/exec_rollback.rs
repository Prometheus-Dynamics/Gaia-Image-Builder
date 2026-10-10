pub mod support;

use gaia_exec::{ExecutionProviders, execute_plan};
use gaia_plan::{plan_build, plan_build_with_reuse_state};
use std::path::Path;
use support::{
    artifact_failure_spec_with_overrides, failing_spec_with_overrides, provider_catalogs,
    reuse_state_for_ids,
};

#[test]
fn rollback_completed_policy_unwinds_completed_outputs_from_current_run() {
    let spec = failing_spec_with_overrides(vec![(
        "policy.failure.rollback_completed".into(),
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
    assert!(!outcome.rolled_back_ids.is_empty());
    // Only unwound operations leave the completed set; an operation whose
    // outputs were kept stays completed and is recorded for reuse.
    assert_eq!(outcome.completed_operations, outcome.completed_ids.len());
    assert!(
        outcome
            .completed_ids
            .iter()
            .all(|id| !outcome.rolled_back_ids.contains(id))
    );
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
    let spec = failing_spec_with_overrides(vec![
        (
            "policy.failure.preserve_failed_outputs".into(),
            "true".into(),
        ),
        ("policy.failure.rollback_completed".into(), "true".into()),
    ]);
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
    let spec = artifact_failure_spec_with_overrides(vec![
        (
            "policy.failure.rollback_domains".into(),
            "artifacts,images,installs,stage,checkpoints".into(),
        ),
        ("policy.failure.rollback_completed".into(), "true".into()),
    ]);
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
    // Source outputs are outside the rolled-back domains: they are kept and
    // stay completed, so they are recorded for reuse.
    assert!(
        outcome
            .completed_ids
            .iter()
            .any(|id| id.as_str() == "source:gaia-upstream"),
        "completed={:?}",
        outcome
            .completed_ids
            .iter()
            .map(|id| id.as_str().to_string())
            .collect::<Vec<_>>()
    );
    assert!(
        !outcome
            .completed_ids
            .iter()
            .any(|id| id.as_str() == "artifact:gaia-app")
    );
}

#[test]
fn failure_keeps_completed_outputs_and_next_plan_reuses_them() {
    let spec = artifact_failure_spec_with_overrides(Vec::new());
    // A real workspace already has its .gaia state directory and Cargo.lock
    // before the first run. Creating them during the run would change the
    // source root's fingerprint, so the next plan would rebuild dependents.
    std::fs::create_dir_all(Path::new(&spec.workspace.root_dir).join(".gaia"))
        .expect("workspace state dir");
    std::fs::write(
        Path::new(&spec.workspace.root_dir).join("Cargo.lock"),
        "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 3\n\n[[package]]\nname = \"gaia\"\nversion = \"2.0.0\"\n",
    )
    .expect("cargo lock");
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let providers = || ExecutionProviders {
        source_catalog: &source_catalog,
        artifact_catalog: &artifact_catalog,
        image_catalog: &image_catalog,
    };

    let outcome = execute_plan(&spec, &plan, providers());

    assert!(!outcome.errors.is_empty());
    assert!(outcome.rolled_back_ids.is_empty());
    assert!(
        outcome
            .completed_ids
            .iter()
            .any(|id| id.as_str() == "artifact:gaia-app"),
        "completed={:?}",
        outcome
            .completed_ids
            .iter()
            .map(|id| id.as_str().to_string())
            .collect::<Vec<_>>()
    );
    assert!(
        Path::new(&spec.workspace.out_dir)
            .join("artifacts/gaia")
            .exists()
    );

    let finished = outcome
        .completed_ids
        .iter()
        .map(|id| id.as_str())
        .collect::<Vec<_>>();
    let mut reuse_state = reuse_state_for_ids(&spec, &plan, &finished);
    // A real save records input signatures too; dependents are rebuilt
    // without them.
    reuse_state.operation_input_signatures = plan
        .operations
        .iter()
        .filter(|operation| finished.contains(&operation.id.as_str()))
        .map(|operation| {
            (
                operation.id.as_str().to_string(),
                gaia_plan::operation_input_signature(&spec, &plan, operation),
            )
        })
        .collect();
    let next_plan = plan_build_with_reuse_state(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(&reuse_state),
    );
    let next = execute_plan(&spec, &next_plan, providers());

    assert!(
        next.reused_ids
            .iter()
            .any(|id| id.as_str() == "artifact:gaia-app"),
        "reused={:?}",
        next.reused_ids
            .iter()
            .map(|id| id.as_str().to_string())
            .collect::<Vec<_>>()
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
