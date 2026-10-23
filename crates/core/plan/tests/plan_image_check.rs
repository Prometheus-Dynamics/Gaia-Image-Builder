//! `image:check` in the plan: planned after the build (or the assembly) for a
//! Buildroot image, left out when `[image.checks] kernel_modules = false`, and
//! reused only while its report and the build it reads are unchanged.
pub mod support;

use gaia_plan::{
    ExecutionPlan, OperationId, OperationKind, OperationReuse, PlannedOperation, RebuildRequest,
    ReuseState, decide_reuse, operation_fingerprint, operation_output_signature, plan_build,
    spec_fingerprint,
};
use gaia_spec::ResolvedBuildSpec;
use gaia_testkit::TempDir;
use std::fs;
use support::{provider_catalogs, test_spec};

fn check_of(plan: &ExecutionPlan) -> Option<&PlannedOperation> {
    plan.operations
        .iter()
        .find(|operation| operation.id.as_str() == "image:check")
}

fn planned(spec: &ResolvedBuildSpec) -> ExecutionPlan {
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    plan_build(spec, &source_catalog, &artifact_catalog, &image_catalog)
}

#[test]
fn a_buildroot_image_plans_its_checks_after_the_build() {
    let scratch = TempDir::new("plan-image-check");
    let spec = test_spec(&scratch);
    let plan = planned(&spec);
    let check = check_of(&plan).expect("image:check is planned");
    assert_eq!(check.kind, OperationKind::CheckImage);
    let expected = if spec
        .image
        .assembly
        .as_ref()
        .is_some_and(|assembly| !assembly.is_empty())
    {
        OperationId::image_assembly()
    } else {
        OperationId::image()
    };
    assert_eq!(check.depends_on, vec![expected]);
    assert!(plan.validate().is_empty(), "{:?}", plan.validate());
}

#[test]
fn kernel_modules_false_leaves_the_checks_out_of_the_plan() {
    let scratch = TempDir::new("plan-image-check-off");
    let mut spec = test_spec(&scratch);
    spec.image.checks.kernel_modules = Some(false);
    assert!(check_of(&planned(&spec)).is_none());
}

/// The build and its check, and nothing else: enough to see the check's own
/// reuse decision without the sources and stages the full plan adds.
fn build_and_check(spec: &ResolvedBuildSpec) -> ExecutionPlan {
    let build = PlannedOperation::new(OperationId::image(), OperationKind::BuildImage)
        .with_fingerprint(operation_fingerprint(spec, &OperationKind::BuildImage));
    let check = PlannedOperation::new(OperationId::image_check(), OperationKind::CheckImage)
        .with_dependency(OperationId::image())
        .with_fingerprint(operation_fingerprint(spec, &OperationKind::CheckImage));
    ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![build, check],
    }
}

/// The reuse state after a run that completed every operation of `plan`.
fn completed_state(spec: &ResolvedBuildSpec, plan: &ExecutionPlan) -> ReuseState {
    ReuseState {
        spec_fingerprint: spec_fingerprint(spec),
        fingerprint_scheme: gaia_spec::FINGERPRINT_SCHEME,
        completed_operation_ids: plan
            .operations
            .iter()
            .map(|operation| operation.id.as_str().to_string())
            .collect(),
        operation_fingerprints: plan
            .operations
            .iter()
            .map(|operation| (operation.id.as_str().to_string(), operation.fingerprint))
            .collect(),
        operation_output_signatures: plan
            .operations
            .iter()
            .filter_map(|operation| {
                operation_output_signature(spec, &operation.kind)
                    .map(|signature| (operation.id.as_str().to_string(), signature))
            })
            .collect(),
        operation_input_signatures: Default::default(),
    }
}

#[test]
fn the_checks_are_reused_while_their_report_and_the_build_are_unchanged() {
    let scratch = TempDir::new("plan-image-check-reuse");
    let mut spec = test_spec(&scratch);
    // The build counts as complete only while its collected image is there.
    let collect = scratch.join("out/images");
    spec.image.output.collect_dir = Some(collect.display().to_string());
    fs::create_dir_all(&collect).expect("collect dir");
    fs::write(collect.join("image-provider.txt"), "built\n").expect("provider marker");
    let report = gaia_spec::image_checks_report_path(&spec);
    fs::create_dir_all(report.parent().expect("out dir")).expect("out dir");
    fs::write(&report, "{\"status\": \"passed\"}\n").expect("report");

    let state = completed_state(&spec, &build_and_check(&spec));
    let reuse = decide_reuse(
        build_and_check(&spec),
        &spec,
        Some(&state),
        &RebuildRequest::default(),
    );
    let check = check_of(&reuse).expect("planned");
    assert!(
        matches!(check.reuse, OperationReuse::Reuse { .. }),
        "{:?}",
        check.reuse
    );

    // A changed report (the check ran again with other results) runs it again.
    fs::write(&report, "{\"status\": \"failed\"}\n").expect("changed report");
    let changed = decide_reuse(
        build_and_check(&spec),
        &spec,
        Some(&state),
        &RebuildRequest::default(),
    );
    match &check_of(&changed).expect("planned").reuse {
        OperationReuse::Execute(reason) => {
            assert_eq!(reason.code, "operation_output_changed", "{reason:?}");
        }
        other => panic!("expected the check to run again, got {other:?}"),
    }
}
