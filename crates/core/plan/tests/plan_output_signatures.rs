pub mod support;

use gaia_plan::{
    OperationReuse, ReuseState, operation_output_signature, plan_build,
    plan_build_with_reuse_state, spec_fingerprint,
};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use support::{provider_catalogs, test_spec};

#[test]
fn plan_rebuilds_when_operation_fingerprint_mismatches_even_if_outputs_exist() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline_plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let reused_ids = ["artifact:gaia-app"];
    if let Some(parent) = PathBuf::from(&spec.artifacts[0].output.path).parent() {
        fs::create_dir_all(parent).expect("artifact output dir");
    }
    fs::write(&spec.artifacts[0].output.path, "artifact").expect("artifact output");
    let state = ReuseState {
        spec_fingerprint: spec_fingerprint(&spec),
        completed_operation_ids: reused_ids
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>(),
        operation_fingerprints: baseline_plan
            .operations
            .iter()
            .filter(|operation| reused_ids.contains(&operation.id.as_str()))
            .map(|operation| (operation.id.as_str().to_string(), operation.fingerprint + 1))
            .collect(),
        operation_output_signatures: baseline_plan
            .operations
            .iter()
            .filter(|operation| reused_ids.contains(&operation.id.as_str()))
            .filter_map(|operation| {
                operation_output_signature(&spec, &operation.kind)
                    .map(|signature| (operation.id.as_str().to_string(), signature))
            })
            .collect(),
    };

    let plan = plan_build_with_reuse_state(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(&state),
    );

    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "artifact:gaia-app"
            && matches!(
                &operation.reuse,
                OperationReuse::Execute(reason) if reason.code == "operation_fingerprint_mismatch"
            )
    }));
}

#[test]
fn plan_rebuilds_when_materialized_outputs_change_even_if_operation_fingerprint_matches() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline_plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let reused_ids = ["artifact:gaia-app"];
    if let Some(parent) = PathBuf::from(&spec.artifacts[0].output.path).parent() {
        fs::create_dir_all(parent).expect("artifact output dir");
    }
    fs::write(&spec.artifacts[0].output.path, "artifact-v1").expect("artifact output");
    let artifact_operation = baseline_plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "artifact:gaia-app")
        .expect("artifact operation");
    let output_signature = operation_output_signature(&spec, &artifact_operation.kind)
        .expect("artifact output signature");

    fs::write(&spec.artifacts[0].output.path, "artifact-v2").expect("artifact output change");

    let state = ReuseState {
        spec_fingerprint: spec_fingerprint(&spec),
        completed_operation_ids: reused_ids
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>(),
        operation_fingerprints: [(
            "artifact:gaia-app".to_string(),
            artifact_operation.fingerprint,
        )]
        .into_iter()
        .collect(),
        operation_output_signatures: [("artifact:gaia-app".to_string(), output_signature)]
            .into_iter()
            .collect(),
    };

    let plan = plan_build_with_reuse_state(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(&state),
    );

    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "artifact:gaia-app"
            && matches!(
                &operation.reuse,
                OperationReuse::Execute(reason) if reason.code == "operation_output_changed"
            )
    }));
}

#[test]
fn plan_rebuilds_when_provider_state_changes_even_if_output_file_matches() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline_plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    if let Some(parent) = PathBuf::from(&spec.artifacts[0].output.path).parent() {
        fs::create_dir_all(parent).expect("artifact output dir");
    }
    fs::write(&spec.artifacts[0].output.path, "artifact-v1").expect("artifact output");
    let state_path = PathBuf::from(&spec.artifacts[0].output.path)
        .parent()
        .expect("artifact parent")
        .join(".gaia/gaia.gaia-state.txt");
    fs::create_dir_all(state_path.parent().expect("state parent")).expect("state dir");
    fs::write(
        &state_path,
        "provider=artifact.rust\nartifact=gaia-app\noutput=one\n",
    )
    .expect("artifact state");
    let artifact_operation = baseline_plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "artifact:gaia-app")
        .expect("artifact operation");
    let output_signature = operation_output_signature(&spec, &artifact_operation.kind)
        .expect("artifact output signature");

    fs::write(
        &state_path,
        "provider=artifact.rust\nartifact=gaia-app\noutput=two\n",
    )
    .expect("artifact state change");

    let state = ReuseState {
        spec_fingerprint: spec_fingerprint(&spec),
        completed_operation_ids: ["artifact:gaia-app"]
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>(),
        operation_fingerprints: [(
            "artifact:gaia-app".to_string(),
            artifact_operation.fingerprint,
        )]
        .into_iter()
        .collect(),
        operation_output_signatures: [("artifact:gaia-app".to_string(), output_signature)]
            .into_iter()
            .collect(),
    };

    let plan = plan_build_with_reuse_state(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(&state),
    );

    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "artifact:gaia-app"
            && matches!(
                &operation.reuse,
                OperationReuse::Execute(reason) if reason.code == "operation_output_changed"
            )
    }));
}
