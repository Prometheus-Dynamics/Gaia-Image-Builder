pub mod support;

use gaia_plan::{
    OperationReuse, ReuseState, operation_output_signature, plan_build,
    plan_build_with_reuse_state, spec_fingerprint,
};
use gaia_spec::{
    AssemblyDiskPartitionSpec, AssemblyDiskSpec, AssemblyFileSpec, AssemblyPartitionTableSpec,
    AssemblyTreeSpec, ImageAssemblySpec,
};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use support::{provider_catalogs, test_spec, test_spec_with_root, unique_dir};

#[test]
fn default_plan_reuses_operations_when_state_matches() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
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
    if let Some(parent) = PathBuf::from(&spec.artifacts[0].output.path).parent() {
        fs::create_dir_all(parent).expect("artifact output dir");
    }
    fs::write(&spec.artifacts[0].output.path, "artifact").expect("artifact output");
    let collect_dir = spec
        .image
        .output
        .collect_dir
        .clone()
        .expect("image collect dir");
    fs::create_dir_all(&collect_dir).expect("image collect dir create");
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
    let runtime_dir = PathBuf::from(&spec.workspace.out_dir).join(".gaia/runtime");
    fs::create_dir_all(&runtime_dir).expect("runtime dir");
    fs::write(
        runtime_dir.join("install-install-gaia-app.state"),
        "kind=install\ninstall_id=install-gaia-app\nartifact_id=gaia-app\n",
    )
    .expect("install runtime state");
    fs::write(
        runtime_dir.join("stage-file-motd.state"),
        "kind=stage-file\nitem_id=motd\n",
    )
    .expect("stage file runtime state");
    fs::write(
        runtime_dir.join("stage-env-runtime-env.state"),
        "kind=stage-env\nitem_id=runtime-env\n",
    )
    .expect("stage env runtime state");
    fs::write(
        runtime_dir.join("stage-service-gaia-service.state"),
        "kind=stage-service\nitem_id=gaia-service\n",
    )
    .expect("stage service runtime state");
    fs::write(
        runtime_dir.join("checkpoint-base-image.state"),
        "kind=checkpoint\ncheckpoint_id=base-image\n",
    )
    .expect("checkpoint runtime state");
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
        operation.id.as_str() == "artifact:gaia-app"
            && matches!(operation.reuse, OperationReuse::Reuse { .. })
    }));
    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "report:emit"
            && matches!(operation.reuse, OperationReuse::Execute(_))
    }));
}

#[test]
fn plan_rebuilds_when_materialized_outputs_are_missing_even_if_state_matches() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline_plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let reused_ids = [
        "source:gaia-upstream",
        "source:workspace-root",
        "artifact:gaia-app",
        "image:build",
    ];
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
        operation.id.as_str() == "artifact:gaia-app"
            && matches!(
                &operation.reuse,
                OperationReuse::Execute(reason) if reason.code == "materialized_output_missing"
            )
    }));
    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "image:build"
            && matches!(
                &operation.reuse,
                OperationReuse::Execute(reason) if reason.code == "materialized_output_missing"
            )
    }));
}

#[test]
fn plan_rebuilds_when_runtime_state_files_are_missing_even_if_other_outputs_exist() {
    let spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
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
    if let Some(parent) = PathBuf::from(&spec.artifacts[0].output.path).parent() {
        fs::create_dir_all(parent).expect("artifact output dir");
    }
    fs::write(&spec.artifacts[0].output.path, "artifact").expect("artifact output");
    let collect_dir = spec
        .image
        .output
        .collect_dir
        .clone()
        .expect("image collect dir");
    fs::create_dir_all(&collect_dir).expect("image collect dir create");
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
    let runtime_dir = PathBuf::from(&spec.workspace.out_dir).join(".gaia/runtime");
    fs::create_dir_all(&runtime_dir).expect("runtime dir");
    fs::write(
        runtime_dir.join("install-install-gaia-app.state"),
        "kind=install\ninstall_id=install-gaia-app\nartifact_id=gaia-app\ndest=/usr/bin/default\n",
    )
    .expect("install runtime state");
    fs::write(
        runtime_dir.join("stage-file-motd.state"),
        "kind=stage-file\nitem_id=motd\ndest=/etc/motd\n",
    )
    .expect("stage file runtime state");
    fs::write(
        runtime_dir.join("stage-env-runtime-env.state"),
        "kind=stage-env\nitem_id=runtime-env\nname=runtime\nentry_count=2\n",
    )
    .expect("stage env runtime state");
    fs::write(
        runtime_dir.join("stage-service-gaia-service.state"),
        "kind=stage-service\nitem_id=gaia-service\nname=gaia.service\n",
    )
    .expect("stage service runtime state");
    fs::write(
        runtime_dir.join("checkpoint-base-image.state"),
        "kind=checkpoint\ncheckpoint_id=base-image\nbackend=local\n",
    )
    .expect("checkpoint runtime state");
    let baseline_plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let reused_ids = [
        "install:install-gaia-app",
        "stage:file:motd",
        "stage:env:runtime-env",
        "stage:service:gaia-service",
        "checkpoint:base-image",
    ];
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

    fs::remove_file(runtime_dir.join("install-install-gaia-app.state"))
        .expect("remove install runtime state");
    fs::remove_file(runtime_dir.join("stage-file-motd.state"))
        .expect("remove stage file runtime state");
    fs::remove_file(runtime_dir.join("checkpoint-base-image.state"))
        .expect("remove checkpoint runtime state");

    let plan = plan_build_with_reuse_state(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(&state),
    );

    for operation_id in [
        "install:install-gaia-app",
        "stage:file:motd",
        "checkpoint:base-image",
    ] {
        assert!(plan.operations.iter().any(|operation| {
            operation.id.as_str() == operation_id
                && matches!(
                    &operation.reuse,
                    OperationReuse::Execute(reason) if reason.code == "materialized_output_missing"
                )
        }));
    }
}

#[test]
fn plan_rebuilds_assembly_when_runtime_state_is_missing_even_if_outputs_exist() {
    let mut spec = test_spec();
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let source_dir = build_dir.join("assembly-source");
    let tree_dir = build_dir.join("assembly-tree");
    fs::create_dir_all(&source_dir).expect("source dir");
    fs::write(source_dir.join("config.txt"), "config").expect("source file");
    fs::create_dir_all(&tree_dir).expect("tree dir");
    fs::write(tree_dir.join("config.txt"), "stale").expect("stale assembly output");
    spec.image.assembly = Some(ImageAssemblySpec {
        trees: vec![AssemblyTreeSpec {
            id: "boot".into(),
            path: tree_dir.display().to_string().into(),
        }],
        files: vec![AssemblyFileSpec {
            tree: "boot".into(),
            src: Some(source_dir.join("config.txt").display().to_string().into()),
            src_glob: None,
            dest: "config.txt".into(),
            mode: None,
            optional: false,
            preserve_symlink: false,
        }],
        ..ImageAssemblySpec::default()
    });
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline_plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let reused_ids = ["image:assembly"];
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
        operation.id.as_str() == "image:assembly"
            && matches!(
                &operation.reuse,
                OperationReuse::Execute(reason) if reason.code == "materialized_output_missing"
            )
    }));
}

#[test]
fn plan_rebuilds_assembly_when_direct_partition_image_changes() {
    let root_dir = unique_dir("gaia-plan-assembly-reuse-partition-root");
    fs::create_dir_all(&root_dir).expect("root dir");
    let mut spec = test_spec_with_root(root_dir);
    let partition_image = gaia_spec::resolve_workspace_path(&spec.workspace, "@assets/rootfs.img")
        .expect("asset path");
    fs::create_dir_all(partition_image.parent().expect("asset parent")).expect("assets dir");
    fs::write(&partition_image, "one").expect("partition image");
    let runtime_dir = PathBuf::from(&spec.workspace.out_dir).join(".gaia/runtime");
    fs::create_dir_all(&runtime_dir).expect("runtime dir");
    fs::write(
        runtime_dir.join("image-assembly.state"),
        "kind=image-assembly\ncompleted_disk_count=1\n",
    )
    .expect("assembly runtime state");
    spec.image.assembly = Some(ImageAssemblySpec {
        disks: vec![AssemblyDiskSpec {
            id: "sdcard".into(),
            output: "$assembly.out/sdcard.img".into(),
            partition_table: AssemblyPartitionTableSpec::Mbr,
            signature: None,
            signature_text: None,
            partitions: vec![AssemblyDiskPartitionSpec {
                name: "rootfs".into(),
                kind: None,
                type_alias: Some("linux".into()),
                bootable: false,
                image: "@assets/rootfs.img".into(),
            }],
        }],
        ..ImageAssemblySpec::default()
    });
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline_plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let reused_ids = ["image:assembly"];
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

    fs::write(&partition_image, "two").expect("updated partition image");
    let plan = plan_build_with_reuse_state(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(&state),
    );

    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "image:assembly"
            && matches!(
                &operation.reuse,
                OperationReuse::Execute(reason) if reason.code == "operation_fingerprint_mismatch"
            )
    }));
}

fn materialize_reusable_test_outputs(spec: &gaia_spec::ResolvedBuildSpec) {
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
    if let Some(parent) = PathBuf::from(&spec.artifacts[0].output.path).parent() {
        fs::create_dir_all(parent).expect("artifact output dir");
    }
    fs::write(&spec.artifacts[0].output.path, "artifact").expect("artifact output");
    let collect_dir = spec
        .image
        .output
        .collect_dir
        .clone()
        .expect("image collect dir");
    fs::create_dir_all(PathBuf::from(&collect_dir).join("buildroot-output/target"))
        .expect("buildroot target dir");
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
    let runtime_dir = PathBuf::from(&spec.workspace.out_dir).join(".gaia/runtime");
    fs::create_dir_all(&runtime_dir).expect("runtime dir");
    fs::write(
        runtime_dir.join("install-install-gaia-app.state"),
        "kind=install\ninstall_id=install-gaia-app\nartifact_id=gaia-app\ndest=/usr/bin/default\n",
    )
    .expect("install runtime state");
    fs::write(
        runtime_dir.join("stage-file-motd.state"),
        "kind=stage-file\nitem_id=motd\ndest=/etc/motd\n",
    )
    .expect("stage file runtime state");
    fs::write(
        runtime_dir.join("stage-env-runtime-env.state"),
        "kind=stage-env\nitem_id=runtime-env\nname=runtime\nentry_count=2\n",
    )
    .expect("stage env runtime state");
    fs::write(
        runtime_dir.join("stage-service-gaia-service.state"),
        "kind=stage-service\nitem_id=gaia-service\nname=gaia.service\n",
    )
    .expect("stage service runtime state");
    fs::write(
        runtime_dir.join("checkpoint-base-image.state"),
        "kind=checkpoint\ncheckpoint_id=base-image\nbackend=local\n",
    )
    .expect("checkpoint runtime state");
}

#[test]
fn stale_spec_fingerprint_still_reuses_unchanged_operation_fingerprints() {
    let mut spec = test_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    materialize_reusable_test_outputs(&spec);
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
    let old_spec_fingerprint = spec_fingerprint(&spec);
    spec.metadata
        .labels
        .push(("release-note".into(), "docs-only".into()));
    let state = ReuseState {
        spec_fingerprint: old_spec_fingerprint,
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
        operation.id.as_str() == "artifact:gaia-app"
            && matches!(&operation.reuse, OperationReuse::Reuse { .. })
    }));
    assert!(plan.operations.iter().any(|operation| {
        operation.id.as_str() == "stage:file:motd"
            && matches!(&operation.reuse, OperationReuse::Reuse { .. })
    }));
}
