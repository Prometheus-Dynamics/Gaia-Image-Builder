pub mod support;

use gaia_exec::{ExecutionProviders, execute_plan};
use gaia_plan::{
    ExecutionPlan, OperationId, OperationKind, OperationReuse, PlannedOperation,
    operation_input_signature,
};
use std::fs;
use std::path::PathBuf;
use support::{provider_catalogs, unique_dir};

fn path_source_spec() -> gaia_spec::ResolvedBuildSpec {
    let mut spec = gaia_spec::ResolvedBuildSpec::new("early-cutoff");
    spec.workspace.root_dir = unique_dir("gaia-exec-cutoff-root");
    spec.workspace.build_dir = unique_dir("gaia-exec-cutoff-build");
    spec.workspace.out_dir = unique_dir("gaia-exec-cutoff-out");
    fs::create_dir_all(&spec.workspace.root_dir).expect("root dir");
    spec.sources = vec![gaia_spec::SourceSpec::new(
        "alpha",
        gaia_spec::SourceDefinition::Path(gaia_spec::PathSourceSpec {
            path: spec.workspace.root_dir.clone(),
            identity_ignore: Vec::new(),
            refresh_policy: gaia_spec::SourceRefreshPolicySpec::Never,
            pin_policy: gaia_spec::SourcePinPolicySpec::Locked,
        }),
    )];
    spec
}

fn plan_with_cutoff(
    spec: &gaia_spec::ResolvedBuildSpec,
    cutoff: impl Fn(&ExecutionPlan, &PlannedOperation) -> Option<u64>,
) -> ExecutionPlan {
    let source_id = spec.sources[0].id.clone();
    let mut plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![
            PlannedOperation::new(OperationId::resolve(), OperationKind::ResolveBuild),
            PlannedOperation::new(
                OperationId::source(&source_id),
                OperationKind::MaterializeSource { source_id },
            )
            .with_dependency(OperationId::resolve())
            .with_reuse(OperationReuse::execute(
                "dependency_rebuilt",
                "dependency is rebuilding",
            )),
        ],
    };
    let signature = cutoff(&plan, &plan.operations[1]);
    plan.operations[1].cutoff_input_signature = signature;
    plan
}

fn materialized_marker(spec: &gaia_spec::ResolvedBuildSpec) -> PathBuf {
    PathBuf::from(&spec.workspace.build_dir).join("sources/alpha/.gaia-source-state.txt")
}

#[test]
fn unchanged_inputs_cut_off_a_dependency_triggered_rebuild() {
    let spec = path_source_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_with_cutoff(&spec, |plan, operation| {
        Some(operation_input_signature(&spec, plan, operation))
    });

    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );

    assert!(outcome.errors.is_empty(), "{:#?}", outcome.errors);
    assert!(
        outcome
            .reused_ids
            .iter()
            .any(|id| id.as_str() == "source:alpha"),
        "{outcome:#?}"
    );
    assert!(!materialized_marker(&spec).exists());
}

#[test]
fn changed_inputs_still_execute() {
    let spec = path_source_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let plan = plan_with_cutoff(&spec, |plan, operation| {
        Some(operation_input_signature(&spec, plan, operation).wrapping_add(1))
    });

    let outcome = execute_plan(
        &spec,
        &plan,
        ExecutionProviders {
            source_catalog: &source_catalog,
            artifact_catalog: &artifact_catalog,
            image_catalog: &image_catalog,
        },
    );

    assert!(outcome.errors.is_empty(), "{:#?}", outcome.errors);
    assert!(
        !outcome
            .reused_ids
            .iter()
            .any(|id| id.as_str() == "source:alpha")
    );
    assert!(materialized_marker(&spec).exists());
}
