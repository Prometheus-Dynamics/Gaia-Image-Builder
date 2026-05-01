pub mod support;

use gaia_plan::{
    OperationReuse, ReuseState, operation_output_signature, plan_build,
    plan_build_with_reuse_state, spec_fingerprint,
};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use support::{provider_catalogs, test_spec_with_root, unique_dir};

#[test]
fn plan_rebuilds_when_path_source_tree_changes() {
    let root_dir = unique_dir("gaia-path-root");
    fs::create_dir_all(&root_dir).expect("root dir");
    fs::write(PathBuf::from(&root_dir).join("tracked.txt"), "one").expect("initial tracked file");

    let spec = test_spec_with_root(root_dir.clone());
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline_plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    fs::create_dir_all(PathBuf::from(&spec.workspace.build_dir).join("sources/gaia-upstream"))
        .expect("gaia-upstream source dir");
    fs::write(
        PathBuf::from(&spec.workspace.build_dir).join("sources/gaia-upstream/source.txt"),
        "ok",
    )
    .expect("gaia-upstream source marker");
    fs::create_dir_all(PathBuf::from(&spec.workspace.build_dir).join("sources/workspace-root"))
        .expect("workspace-root source dir");
    fs::write(
        PathBuf::from(&spec.workspace.build_dir).join("sources/workspace-root/source.txt"),
        "ok",
    )
    .expect("workspace-root source marker");

    fs::write(PathBuf::from(&root_dir).join("tracked.txt"), "two").expect("updated tracked file");

    let reused_ids = ["source:gaia-upstream", "source:workspace-root"];
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
            .map(|operation| (operation.id.as_str().to_string(), operation.fingerprint))
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
        operation.id.as_str() == "source:workspace-root"
            && matches!(
                &operation.reuse,
                OperationReuse::Execute(reason) if reason.code == "operation_fingerprint_mismatch"
            )
    }));
}

#[test]
fn plan_ignores_workspace_derived_dirs_when_hashing_path_sources() {
    let root = PathBuf::from(unique_dir("gaia-plan-root"));
    fs::create_dir_all(&root).expect("root dir");
    fs::write(root.join("tracked.txt"), "v1").expect("tracked file");
    fs::create_dir_all(root.join("out")).expect("workspace out dir");
    fs::write(root.join("out/runtime.log"), "before").expect("workspace out file");

    let spec = test_spec_with_root(root.display().to_string());
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline_plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let source_operation = baseline_plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "source:workspace-root")
        .expect("workspace-root operation");

    fs::write(root.join("out/runtime.log"), "after").expect("updated workspace out file");

    let source_dir = PathBuf::from(&spec.workspace.build_dir).join("sources/workspace-root");
    fs::create_dir_all(&source_dir).expect("workspace-root source dir");
    fs::write(source_dir.join("source.txt"), "ok").expect("workspace-root source marker");
    fs::write(
        source_dir.join(".gaia-source-state.txt"),
        "provider=source.path\npath_digest=abc123\ncontent_identity_mode=live-reference\n",
    )
    .expect("workspace-root source state");

    let state = ReuseState {
        spec_fingerprint: spec_fingerprint(&spec),
        completed_operation_ids: ["source:workspace-root"]
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>(),
        operation_fingerprints: [(
            "source:workspace-root".to_string(),
            source_operation.fingerprint,
        )]
        .into_iter()
        .collect(),
        operation_output_signatures: [(
            "source:workspace-root".to_string(),
            operation_output_signature(&spec, &source_operation.kind)
                .expect("source output signature"),
        )]
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
        operation.id.as_str() == "source:workspace-root"
            && matches!(operation.reuse, OperationReuse::Reuse { .. })
    }));
}

#[test]
fn plan_rebuilds_source_when_provider_state_changes() {
    let root_dir = unique_dir("gaia-source-state-drift-root");
    let spec = test_spec_with_root(root_dir);
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline_plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let source_dir = PathBuf::from(&spec.workspace.build_dir).join("sources/workspace-root");
    fs::create_dir_all(&source_dir).expect("workspace-root source dir");
    fs::write(source_dir.join("source.txt"), "ok").expect("workspace-root source marker");
    fs::write(
        source_dir.join(".gaia-source-state.txt"),
        "provider=source.path\npath_digest=abc123\ncontent_identity_mode=live-reference\n",
    )
    .expect("workspace-root source state");

    let source_operation = baseline_plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == "source:workspace-root")
        .expect("workspace-root operation");
    let baseline_signature = operation_output_signature(&spec, &source_operation.kind)
        .expect("baseline source output signature");

    fs::write(
        source_dir.join(".gaia-source-state.txt"),
        "provider=source.path\npath_digest=abc123\ncontent_identity_mode=refreshed-snapshot\n",
    )
    .expect("updated workspace-root source state");

    let state = ReuseState {
        spec_fingerprint: spec_fingerprint(&spec),
        completed_operation_ids: ["source:workspace-root"]
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>(),
        operation_fingerprints: [(
            "source:workspace-root".to_string(),
            source_operation.fingerprint,
        )]
        .into_iter()
        .collect(),
        operation_output_signatures: [("source:workspace-root".to_string(), baseline_signature)]
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
        operation.id.as_str() == "source:workspace-root"
            && matches!(
                &operation.reuse,
                OperationReuse::Execute(reason) if reason.code == "operation_output_changed"
            )
    }));
}
