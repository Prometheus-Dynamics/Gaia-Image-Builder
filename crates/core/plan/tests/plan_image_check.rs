//! `image:check` in the plan: planned after the build (or the assembly) for a
//! Buildroot image, left out when `[image.checks] kernel_modules = false`
//! unless an installed surface library needs its consumers checked (which a
//! starting-point image plans it for too), and reused only while its report
//! and the build it reads are unchanged.
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

/// Gives every installed Rust artifact a declared surface.
fn install_a_surface(spec: &mut ResolvedBuildSpec) {
    let installed = spec
        .install
        .entries
        .iter()
        .map(|entry| entry.artifact.id.clone())
        .collect::<Vec<_>>();
    let mut declared = 0;
    for artifact in &mut spec.artifacts {
        if let gaia_spec::ArtifactDefinition::Rust(rust) = &mut artifact.definition
            && installed.contains(&artifact.id)
        {
            rust.surface = Some(gaia_spec::RustSurfaceSpec {
                exports: "surface.map".into(),
            });
            declared += 1;
        }
    }
    assert!(declared > 0, "the test config installs a Rust artifact");
}

#[test]
fn an_installed_surface_plans_the_checks_with_the_kernel_checks_off() {
    let scratch = TempDir::new("plan-image-check-surface");
    let mut spec = test_spec(&scratch);
    spec.image.checks.kernel_modules = Some(false);
    install_a_surface(&mut spec);
    assert!(check_of(&planned(&spec)).is_some());
}

#[test]
fn a_starting_point_image_plans_the_checks_only_for_an_installed_surface() {
    let scratch = TempDir::new("plan-image-check-starting-point");
    let mut spec = test_spec(&scratch);
    spec.image.definition = gaia_spec::ImageDefinition::StartingPoint(Default::default());
    spec.image.assembly = None;
    assert!(check_of(&planned(&spec)).is_none());
    install_a_surface(&mut spec);
    let plan = planned(&spec);
    let check = check_of(&plan).expect("image:check is planned");
    assert_eq!(check.depends_on, vec![OperationId::image()]);
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
    let current = format!(
        "{{\"schema\": {}, \"status\": \"passed\"}}\n",
        gaia_spec::IMAGE_CHECKS_REPORT_SCHEMA
    );
    fs::write(&report, &current).expect("report");

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
    let changed_report = format!(
        "{{\"schema\": {}, \"status\": \"failed\"}}\n",
        gaia_spec::IMAGE_CHECKS_REPORT_SCHEMA
    );
    fs::write(&report, &changed_report).expect("changed report");
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

#[test]
fn a_report_of_an_older_schema_runs_the_checks_again() {
    let scratch = TempDir::new("plan-image-check-schema");
    let mut spec = test_spec(&scratch);
    let collect = scratch.join("out/images");
    spec.image.output.collect_dir = Some(collect.display().to_string());
    fs::create_dir_all(&collect).expect("collect dir");
    fs::write(collect.join("image-provider.txt"), "built\n").expect("provider marker");
    let report = gaia_spec::image_checks_report_path(&spec);
    fs::create_dir_all(report.parent().expect("out dir")).expect("out dir");
    fs::write(&report, "{\"schema\": 1, \"status\": \"passed\"}\n").expect("report");

    let state = completed_state(&spec, &build_and_check(&spec));
    let reuse = decide_reuse(
        build_and_check(&spec),
        &spec,
        Some(&state),
        &RebuildRequest::default(),
    );
    assert!(
        matches!(
            check_of(&reuse).expect("planned").reuse,
            OperationReuse::Execute(_)
        ),
        "an older report is not reused"
    );
}
