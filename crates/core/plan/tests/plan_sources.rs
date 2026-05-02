pub mod support;

use gaia_plan::{
    OperationReuse, ReuseState, operation_output_signature, plan_build,
    plan_build_with_reuse_state, spec_fingerprint,
};
use gaia_spec::{SourceDefinition, SourceRefreshPolicySpec};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use support::{provider_catalogs, test_spec, test_spec_with_root, unique_dir};

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

#[test]
fn plan_rebuilds_artifact_when_declared_path_source_changes() {
    let root_dir = unique_dir("gaia-artifact-source-root");
    fs::create_dir_all(&root_dir).expect("root dir");
    fs::write(PathBuf::from(&root_dir).join("Cargo.toml"), "version = 1\n")
        .expect("initial source file");

    let spec = test_spec_with_root(root_dir.clone());
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline_plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let source_dir = PathBuf::from(&spec.workspace.build_dir).join("sources/workspace-root");
    fs::create_dir_all(&source_dir).expect("workspace-root source dir");
    fs::write(source_dir.join("source.txt"), "ok").expect("workspace-root source marker");
    if let Some(parent) = PathBuf::from(&spec.artifacts[0].output.path).parent() {
        fs::create_dir_all(parent).expect("artifact output dir");
    }
    fs::write(&spec.artifacts[0].output.path, "artifact").expect("artifact output");

    fs::write(PathBuf::from(&root_dir).join("Cargo.toml"), "version = 2\n")
        .expect("updated source file");

    let state = support::reuse_state_for_ids(
        &spec,
        &baseline_plan,
        &["source:workspace-root", "artifact:gaia-app"],
    );
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
    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "artifact:gaia-app"
            && matches!(
                &operation.reuse,
                OperationReuse::Execute(reason) if reason.code == "dependency_rebuilt"
            )
    }));
}

#[test]
fn plan_rebuilds_sources_with_always_refresh_policy_even_when_state_matches() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline_plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let source_dir = PathBuf::from(&spec.workspace.build_dir).join("sources/gaia-upstream");
    fs::create_dir_all(&source_dir).expect("gaia-upstream source dir");
    fs::write(source_dir.join("source.txt"), "ok").expect("gaia-upstream source marker");
    fs::write(
        source_dir.join(".gaia-source-state.txt"),
        "provider=source.git\nresolved_commit_sha=abc123\n",
    )
    .expect("gaia-upstream source state");

    let state = support::reuse_state_for_ids(&spec, &baseline_plan, &["source:gaia-upstream"]);
    let plan = plan_build_with_reuse_state(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(&state),
    );

    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "source:gaia-upstream"
            && matches!(
                &operation.reuse,
                OperationReuse::Execute(reason) if reason.code == "source_refresh_always"
            )
    }));
}

#[test]
fn plan_rebuilds_auto_floating_remote_git_sources() {
    let mut spec = test_spec();
    let source = spec
        .sources
        .iter_mut()
        .find(|source| source.id.as_str() == "gaia-upstream")
        .expect("gaia-upstream source");
    if let SourceDefinition::Git(git) = &mut source.definition {
        git.refresh_policy = SourceRefreshPolicySpec::Auto;
    }
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline_plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let source_dir = PathBuf::from(&spec.workspace.build_dir).join("sources/gaia-upstream");
    fs::create_dir_all(&source_dir).expect("gaia-upstream source dir");
    fs::write(source_dir.join("source.txt"), "ok").expect("gaia-upstream source marker");
    fs::write(
        source_dir.join(".gaia-source-state.txt"),
        "provider=source.git\nresolved_commit_sha=abc123\n",
    )
    .expect("gaia-upstream source state");

    let state = support::reuse_state_for_ids(&spec, &baseline_plan, &["source:gaia-upstream"]);
    let plan = plan_build_with_reuse_state(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(&state),
    );

    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "source:gaia-upstream"
            && matches!(
                &operation.reuse,
                OperationReuse::Execute(reason) if reason.code == "remote_floating_source"
            )
    }));
}

#[test]
fn plan_rebuilds_stage_file_and_image_when_stage_asset_changes() {
    let root_dir = unique_dir("gaia-stage-asset-root");
    let root = PathBuf::from(&root_dir);
    let assets = root.join("examples/default-workspace/assets");
    fs::create_dir_all(assets.join("etc")).expect("asset dir");
    fs::create_dir_all(assets.join("systemd")).expect("service asset dir");
    fs::write(assets.join("etc/motd"), "hello\n").expect("motd asset");
    fs::write(
        assets.join("systemd/gaia.service"),
        "[Service]\nExecStart=/bin/true\n",
    )
    .expect("service asset");

    let spec = test_spec_with_root(root_dir);
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline_plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let runtime_dir = PathBuf::from(&spec.workspace.out_dir).join(".gaia/runtime");
    fs::create_dir_all(&runtime_dir).expect("runtime dir");
    fs::write(
        runtime_dir.join("stage-file-motd.state"),
        "kind=stage-file\nitem_id=motd\ndest=/etc/motd\n",
    )
    .expect("stage file runtime state");
    let collect_dir = spec
        .image
        .output
        .collect_dir
        .clone()
        .expect("image collect dir");
    fs::create_dir_all(&collect_dir).expect("image collect dir");
    fs::write(
        PathBuf::from(&collect_dir).join("image-provider.txt"),
        "image",
    )
    .expect("image marker");
    let archive_name = spec
        .image
        .output
        .archive_name
        .clone()
        .expect("image archive name");
    fs::write(PathBuf::from(&collect_dir).join(archive_name), "archive").expect("image archive");

    let state =
        support::reuse_state_for_ids(&spec, &baseline_plan, &["stage:file:motd", "image:build"]);
    fs::write(assets.join("etc/motd"), "hello again\n").expect("updated motd asset");

    let plan = plan_build_with_reuse_state(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(&state),
    );

    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "stage:file:motd"
            && matches!(
                &operation.reuse,
                OperationReuse::Execute(reason) if reason.code == "operation_fingerprint_mismatch"
            )
    }));
    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "image:build"
            && matches!(
                &operation.reuse,
                OperationReuse::Execute(reason) if reason.code == "dependency_rebuilt"
            )
    }));
}

#[test]
#[cfg(unix)]
fn plan_rebuilds_stage_service_when_unit_mode_changes() {
    use std::os::unix::fs::PermissionsExt;

    let root_dir = unique_dir("gaia-stage-service-mode-root");
    let root = PathBuf::from(&root_dir);
    let assets = root.join("examples/default-workspace/assets");
    fs::create_dir_all(assets.join("etc")).expect("asset dir");
    fs::create_dir_all(assets.join("systemd")).expect("service asset dir");
    fs::write(assets.join("etc/motd"), "hello\n").expect("motd asset");
    let service_path = assets.join("systemd/gaia.service");
    fs::write(&service_path, "[Service]\nExecStart=/bin/true\n").expect("service asset");
    fs::set_permissions(&service_path, fs::Permissions::from_mode(0o644))
        .expect("initial service mode");

    let spec = test_spec_with_root(root_dir);
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline_plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    let runtime_dir = PathBuf::from(&spec.workspace.out_dir).join(".gaia/runtime");
    fs::create_dir_all(&runtime_dir).expect("runtime dir");
    fs::write(
        runtime_dir.join("stage-service-gaia-service.state"),
        "kind=stage-service\nitem_id=gaia-service\nname=gaia.service\n",
    )
    .expect("stage service runtime state");

    let state =
        support::reuse_state_for_ids(&spec, &baseline_plan, &["stage:service:gaia-service"]);
    fs::set_permissions(&service_path, fs::Permissions::from_mode(0o755))
        .expect("updated service mode");

    let plan = plan_build_with_reuse_state(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(&state),
    );

    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "stage:service:gaia-service"
            && matches!(
                &operation.reuse,
                OperationReuse::Execute(reason) if reason.code == "operation_fingerprint_mismatch"
            )
    }));
}
