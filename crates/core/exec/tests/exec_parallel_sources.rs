pub mod support;

use gaia_artifact_providers::ArtifactProviderCatalog;
use gaia_exec::{ExecutionProviders, execute_plan};
use gaia_image_providers::ImageProviderCatalog;
use gaia_plan::{
    ExecutionPlan, OperationId, OperationKind, OperationOptionality, OperationParallelism,
    OperationParallelismDomain, OperationReuse, PlannedOperation,
};
use gaia_source_providers::SourceProviderCatalog;
use std::fs;
use std::time::{Duration, Instant};
use support::{FailThenCancelAwarePathSourceProvider, SleepPathSourceProvider, unique_dir};

#[test]
fn parallelizable_source_operations_execute_concurrently() {
    let mut spec = gaia_spec::ResolvedBuildSpec::new("parallel-exec");
    spec.workspace.root_dir = unique_dir("gaia-exec-parallel-root");
    spec.workspace.build_dir = unique_dir("gaia-exec-parallel-build");
    spec.workspace.out_dir = unique_dir("gaia-exec-parallel-out");
    fs::create_dir_all(&spec.workspace.root_dir).expect("parallel root dir");
    spec.sources = vec![
        gaia_spec::SourceSpec::new(
            "alpha",
            gaia_spec::SourceDefinition::Path(gaia_spec::PathSourceSpec {
                path: spec.workspace.root_dir.clone(),
                identity_ignore: Vec::new(),
                refresh_policy: gaia_spec::SourceRefreshPolicySpec::Never,
                pin_policy: gaia_spec::SourcePinPolicySpec::Locked,
            }),
        ),
        gaia_spec::SourceSpec::new(
            "beta",
            gaia_spec::SourceDefinition::Path(gaia_spec::PathSourceSpec {
                path: spec.workspace.root_dir.clone(),
                identity_ignore: Vec::new(),
                refresh_policy: gaia_spec::SourceRefreshPolicySpec::Never,
                pin_policy: gaia_spec::SourcePinPolicySpec::Locked,
            }),
        ),
    ];

    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![
            PlannedOperation::new(OperationId::resolve(), OperationKind::ResolveBuild)
                .with_parallelism(OperationParallelism::exclusive(
                    OperationParallelismDomain::Global,
                ))
                .with_optionality(OperationOptionality::Required)
                .with_reuse(OperationReuse::execute("resolve", "resolve")),
            PlannedOperation::new(
                OperationId::source(&spec.sources[0].id),
                OperationKind::MaterializeSource {
                    source_id: spec.sources[0].id.clone(),
                },
            )
            .with_dependency(OperationId::resolve())
            .with_parallelism(OperationParallelism::parallelizable(
                OperationParallelismDomain::Sources,
            ))
            .with_optionality(OperationOptionality::Required)
            .with_reuse(OperationReuse::execute("source", "source")),
            PlannedOperation::new(
                OperationId::source(&spec.sources[1].id),
                OperationKind::MaterializeSource {
                    source_id: spec.sources[1].id.clone(),
                },
            )
            .with_dependency(OperationId::resolve())
            .with_parallelism(OperationParallelism::parallelizable(
                OperationParallelismDomain::Sources,
            ))
            .with_optionality(OperationOptionality::Required)
            .with_reuse(OperationReuse::execute("source", "source")),
        ],
    };

    let mut source_catalog = SourceProviderCatalog::new();
    source_catalog.register(Box::new(SleepPathSourceProvider));
    let artifact_catalog = ArtifactProviderCatalog::new();
    let image_catalog = ImageProviderCatalog::new();

    let started = Instant::now();
    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );
    let elapsed = started.elapsed();

    assert!(outcome.errors.is_empty());
    assert_eq!(outcome.completed_operations, 3);
    assert!(elapsed < Duration::from_millis(525), "elapsed: {elapsed:?}");
}

#[test]
fn failed_parallel_operation_cancels_running_siblings_before_rollback() {
    let mut spec = gaia_spec::ResolvedBuildSpec::new("parallel-failure-cancels-siblings");
    spec.workspace.root_dir = unique_dir("gaia-exec-parallel-fail-root");
    spec.workspace.build_dir = unique_dir("gaia-exec-parallel-fail-build");
    spec.workspace.out_dir = unique_dir("gaia-exec-parallel-fail-out");
    spec.policy.execution.jobs = 2;
    fs::create_dir_all(&spec.workspace.root_dir).expect("parallel fail root dir");
    spec.sources = vec![
        gaia_spec::SourceSpec::new(
            "fail",
            gaia_spec::SourceDefinition::Path(gaia_spec::PathSourceSpec {
                path: spec.workspace.root_dir.clone(),
                identity_ignore: Vec::new(),
                refresh_policy: gaia_spec::SourceRefreshPolicySpec::Never,
                pin_policy: gaia_spec::SourcePinPolicySpec::Locked,
            }),
        ),
        gaia_spec::SourceSpec::new(
            "slow",
            gaia_spec::SourceDefinition::Path(gaia_spec::PathSourceSpec {
                path: spec.workspace.root_dir.clone(),
                identity_ignore: Vec::new(),
                refresh_policy: gaia_spec::SourceRefreshPolicySpec::Never,
                pin_policy: gaia_spec::SourcePinPolicySpec::Locked,
            }),
        ),
    ];

    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![
            PlannedOperation::new(OperationId::resolve(), OperationKind::ResolveBuild)
                .with_parallelism(OperationParallelism::exclusive(
                    OperationParallelismDomain::Global,
                ))
                .with_optionality(OperationOptionality::Required)
                .with_reuse(OperationReuse::execute("resolve", "resolve")),
            PlannedOperation::new(
                OperationId::source(&spec.sources[0].id),
                OperationKind::MaterializeSource {
                    source_id: spec.sources[0].id.clone(),
                },
            )
            .with_dependency(OperationId::resolve())
            .with_parallelism(OperationParallelism::parallelizable(
                OperationParallelismDomain::Sources,
            ))
            .with_optionality(OperationOptionality::Required)
            .with_reuse(OperationReuse::execute("source", "source")),
            PlannedOperation::new(
                OperationId::source(&spec.sources[1].id),
                OperationKind::MaterializeSource {
                    source_id: spec.sources[1].id.clone(),
                },
            )
            .with_dependency(OperationId::resolve())
            .with_parallelism(OperationParallelism::parallelizable(
                OperationParallelismDomain::Sources,
            ))
            .with_optionality(OperationOptionality::Required)
            .with_reuse(OperationReuse::execute("source", "source")),
        ],
    };

    let mut source_catalog = SourceProviderCatalog::new();
    source_catalog.register(Box::new(FailThenCancelAwarePathSourceProvider));
    let artifact_catalog = ArtifactProviderCatalog::new();
    let image_catalog = ImageProviderCatalog::new();

    let started = Instant::now();
    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );
    let elapsed = started.elapsed();

    assert_eq!(outcome.errors.len(), 1);
    assert_eq!(
        outcome.errors[0].operation_id.as_str(),
        "source:fail",
        "errors: {:?}",
        outcome.errors
    );
    assert!(
        elapsed < Duration::from_millis(500),
        "sibling operation was not cancelled promptly: {elapsed:?}"
    );
    assert!(
        !std::path::Path::new(&spec.workspace.build_dir)
            .join("sources/slow")
            .exists()
    );
}
