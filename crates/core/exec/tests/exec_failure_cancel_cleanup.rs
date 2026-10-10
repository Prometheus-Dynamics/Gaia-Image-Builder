pub mod support;

use gaia_artifact_providers::ArtifactProviderCatalog;
use gaia_exec::OperationTimingStatus;
use gaia_exec::{ExecutionProviders, execute_plan};
use gaia_image_providers::ImageProviderCatalog;
use gaia_plan::{
    ExecutionPlan, OperationId, OperationKind, OperationOptionality, OperationParallelism,
    OperationParallelismDomain, OperationReuse, PlannedOperation,
};
use gaia_source_providers::SourceProviderCatalog;
use std::fs;
use std::path::{Path, PathBuf};
use support::{FailAndLeavePartialSourceProvider, unique_dir};

/// Two parallel sources: `fail` fails, `slow` is in flight and gets cancelled
/// by the stop signal while it has written a partial output.
fn fail_and_slow_spec(name: &str, preserve_failed_outputs: bool) -> gaia_spec::ResolvedBuildSpec {
    let mut spec = gaia_spec::ResolvedBuildSpec::new(name);
    spec.workspace.root_dir = unique_dir(&format!("gaia-exec-{name}-root"));
    spec.workspace.build_dir = unique_dir(&format!("gaia-exec-{name}-build"));
    spec.workspace.out_dir = unique_dir(&format!("gaia-exec-{name}-out"));
    spec.policy.execution.jobs = 2;
    spec.policy.failure.preserve_failed_outputs = preserve_failed_outputs;
    fs::create_dir_all(&spec.workspace.root_dir).expect("root dir");
    spec.sources = ["fail", "slow"]
        .into_iter()
        .map(|id| {
            gaia_spec::SourceSpec::new(
                id,
                gaia_spec::SourceDefinition::Path(gaia_spec::PathSourceSpec {
                    path: spec.workspace.root_dir.clone(),
                    identity_ignore: Vec::new(),
                    refresh_policy: gaia_spec::SourceRefreshPolicySpec::Never,
                    pin_policy: gaia_spec::SourcePinPolicySpec::Locked,
                }),
            )
        })
        .collect();
    spec
}

fn fail_and_slow_plan(spec: &gaia_spec::ResolvedBuildSpec) -> ExecutionPlan {
    let mut operations = vec![
        PlannedOperation::new(OperationId::resolve(), OperationKind::ResolveBuild)
            .with_parallelism(OperationParallelism::exclusive(
                OperationParallelismDomain::Global,
            ))
            .with_optionality(OperationOptionality::Required)
            .with_reuse(OperationReuse::execute("resolve", "resolve")),
    ];
    for source in &spec.sources {
        operations.push(
            PlannedOperation::new(
                OperationId::source(&source.id),
                OperationKind::MaterializeSource {
                    source_id: source.id.clone(),
                },
            )
            .with_dependency(OperationId::resolve())
            .with_parallelism(OperationParallelism::parallelizable(
                OperationParallelismDomain::Sources,
            ))
            .with_optionality(OperationOptionality::Required)
            .with_reuse(OperationReuse::execute("source", "source")),
        );
    }
    ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations,
    }
}

fn run_fail_and_slow(spec: &gaia_spec::ResolvedBuildSpec) -> gaia_exec::ExecutionOutcome {
    let plan = fail_and_slow_plan(spec);
    let mut source_catalog = SourceProviderCatalog::new();
    source_catalog.register(Box::new(FailAndLeavePartialSourceProvider));
    let artifact_catalog = ArtifactProviderCatalog::new();
    let image_catalog = ImageProviderCatalog::new();
    execute_plan(
        spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    )
}

fn slow_output_dir(spec: &gaia_spec::ResolvedBuildSpec) -> PathBuf {
    Path::new(&spec.workspace.build_dir).join("sources/slow")
}

#[test]
fn cancelled_sibling_partial_output_is_removed_and_not_recorded_as_completed() {
    let spec = fail_and_slow_spec("exec-sibling-partial", false);

    let outcome = run_fail_and_slow(&spec);

    assert_eq!(outcome.errors.len(), 1, "{:#?}", outcome.errors);
    assert_eq!(outcome.errors[0].operation_id.as_str(), "source:fail");
    assert!(
        !slow_output_dir(&spec).exists(),
        "cancelled sibling's partial output should be removed"
    );
    assert!(
        !outcome
            .completed_ids
            .iter()
            .any(|id| id.as_str() == "source:slow"),
        "a cancelled operation must not be recorded as completed"
    );
    // The sibling's provider error has kind Cancelled. It is reported as a
    // cancellation, not as a second failure.
    let slow_timing = outcome
        .operation_timings
        .iter()
        .find(|timing| timing.operation_id.as_str() == "source:slow")
        .expect("timing for source:slow");
    assert_eq!(slow_timing.status, OperationTimingStatus::Cancelled);
}

#[test]
fn cancelled_sibling_partial_output_is_kept_when_failed_outputs_are_preserved() {
    let spec = fail_and_slow_spec("exec-sibling-preserve", true);

    let outcome = run_fail_and_slow(&spec);

    assert_eq!(outcome.errors.len(), 1, "{:#?}", outcome.errors);
    assert!(
        slow_output_dir(&spec).join("partial.txt").exists(),
        "preserve_failed_outputs should keep the cancelled sibling's partial output"
    );
    assert!(
        !outcome
            .completed_ids
            .iter()
            .any(|id| id.as_str() == "source:slow"),
        "a cancelled operation must not be recorded as completed"
    );
}
