use super::*;
use gaia_plan::{OperationKind, OperationParallelism, OperationReuse};
use gaia_spec::{
    ArtifactDefinition, ArtifactId, ArtifactOutputSpec, ArtifactRef, ArtifactSpec,
    ArtifactVariantSpec, BuildrootImageSpec, CheckpointId, ImageDefinition, ImageSpec,
    InstallEntrySpec, InstallId, NodeArtifactSpec, PathSourceSpec, RustArtifactSpec,
    SourceDefinition, SourcePinPolicySpec, SourceRef, SourceRefreshPolicySpec, SourceSpec,
    StageContentOriginSpec, StageFileSpec, StageItemId, StageServiceSpec,
};

fn parallel_operation(
    id: OperationId,
    kind: OperationKind,
    domain: OperationParallelismDomain,
) -> PlannedOperation {
    PlannedOperation::new(id, kind)
        .with_parallelism(OperationParallelism::parallelizable(domain))
        .with_reuse(OperationReuse::execute("test", "test operation"))
}

fn parallel_runtime_operation(id: OperationId, kind: OperationKind) -> PlannedOperation {
    parallel_operation(id, kind, OperationParallelismDomain::Runtime)
}

fn parallel_artifact_operation(id: OperationId, kind: OperationKind) -> PlannedOperation {
    parallel_operation(id, kind, OperationParallelismDomain::Artifacts)
}

fn path_source(id: &str, path: &str) -> SourceSpec {
    SourceSpec::new(
        id,
        SourceDefinition::Path(PathSourceSpec {
            path: path.into(),
            identity_ignore: Vec::new(),
            refresh_policy: SourceRefreshPolicySpec::Never,
            pin_policy: SourcePinPolicySpec::Locked,
        }),
    )
}

fn rust_artifact(id: &str, source: &str, output: &str) -> ArtifactSpec {
    ArtifactSpec::new(
        id,
        ArtifactDefinition::Rust(RustArtifactSpec {
            package: id.into(),
            target_name: None,
            variant: ArtifactVariantSpec::File,
        }),
        Some(SourceRef::new(source)),
        ArtifactOutputSpec {
            path: output.into(),
        },
    )
}

fn node_artifact(id: &str, source: &str, package_dir: &str, output: &str) -> ArtifactSpec {
    ArtifactSpec::new(
        id,
        ArtifactDefinition::Node(NodeArtifactSpec {
            package_dir: package_dir.into(),
        }),
        Some(SourceRef::new(source)),
        ArtifactOutputSpec {
            path: output.into(),
        },
    )
}

fn assert_second_operation_blocked(spec: &ResolvedBuildSpec, plan: &ExecutionPlan) {
    assert_eq!(
        next_schedulable_operation(spec, plan, &[0, 0], &[false, false], &[true, false]),
        None
    );
}

#[test]
fn scheduler_blocks_parallel_stage_files_with_same_destination() {
    let mut spec = ResolvedBuildSpec::new("parallel-resource-test");
    spec.stage.files = vec![
        StageFileSpec {
            id: StageItemId::new("motd-a"),
            src: "a".into(),
            dest: "/etc/motd".into(),
            origin: StageContentOriginSpec::StaticAsset,
        },
        StageFileSpec {
            id: StageItemId::new("motd-b"),
            src: "b".into(),
            dest: "/etc/motd".into(),
            origin: StageContentOriginSpec::StaticAsset,
        },
    ];
    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![
            parallel_runtime_operation(
                OperationId::stage_file(&spec.stage.files[0].id),
                OperationKind::RenderStageFile {
                    item_id: spec.stage.files[0].id.clone(),
                },
            ),
            parallel_runtime_operation(
                OperationId::stage_file(&spec.stage.files[1].id),
                OperationKind::RenderStageFile {
                    item_id: spec.stage.files[1].id.clone(),
                },
            ),
        ],
    };

    assert_second_operation_blocked(&spec, &plan);
}

#[test]
fn scheduler_allows_parallel_stage_files_with_different_destinations() {
    let mut spec = ResolvedBuildSpec::new("parallel-resource-test");
    spec.stage.files = vec![
        StageFileSpec {
            id: StageItemId::new("motd"),
            src: "a".into(),
            dest: "/etc/motd".into(),
            origin: StageContentOriginSpec::StaticAsset,
        },
        StageFileSpec {
            id: StageItemId::new("issue"),
            src: "b".into(),
            dest: "/etc/issue".into(),
            origin: StageContentOriginSpec::StaticAsset,
        },
    ];
    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![
            parallel_runtime_operation(
                OperationId::stage_file(&spec.stage.files[0].id),
                OperationKind::RenderStageFile {
                    item_id: spec.stage.files[0].id.clone(),
                },
            ),
            parallel_runtime_operation(
                OperationId::stage_file(&spec.stage.files[1].id),
                OperationKind::RenderStageFile {
                    item_id: spec.stage.files[1].id.clone(),
                },
            ),
        ],
    };

    assert_eq!(
        next_schedulable_operation(&spec, &plan, &[0, 0], &[false, false], &[true, false]),
        Some(1)
    );
}

#[test]
fn scheduler_blocks_parallel_artifacts_with_same_output_path() {
    let mut spec = ResolvedBuildSpec::new("parallel-resource-test");
    spec.sources = vec![
        path_source("source-a", "source-a"),
        path_source("source-b", "source-b"),
    ];
    spec.artifacts = vec![
        rust_artifact("artifact-a", "source-a", "out/bin/app"),
        rust_artifact("artifact-b", "source-b", "out/bin/app"),
    ];
    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![
            parallel_artifact_operation(
                OperationId::artifact(&spec.artifacts[0].id),
                OperationKind::BuildArtifact {
                    artifact_id: spec.artifacts[0].id.clone(),
                },
            ),
            parallel_artifact_operation(
                OperationId::artifact(&spec.artifacts[1].id),
                OperationKind::BuildArtifact {
                    artifact_id: spec.artifacts[1].id.clone(),
                },
            ),
        ],
    };

    assert_second_operation_blocked(&spec, &plan);
}

#[test]
fn scheduler_blocks_parallel_artifacts_with_same_build_input() {
    let mut spec = ResolvedBuildSpec::new("parallel-resource-test");
    spec.sources = vec![path_source("workspace", "workspace")];
    spec.artifacts = vec![
        node_artifact("node-a", "workspace", "packages/app", "out/a.tgz"),
        node_artifact("node-b", "workspace", "packages/app", "out/b.tgz"),
    ];
    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![
            parallel_artifact_operation(
                OperationId::artifact(&spec.artifacts[0].id),
                OperationKind::BuildArtifact {
                    artifact_id: spec.artifacts[0].id.clone(),
                },
            ),
            parallel_artifact_operation(
                OperationId::artifact(&spec.artifacts[1].id),
                OperationKind::BuildArtifact {
                    artifact_id: spec.artifacts[1].id.clone(),
                },
            ),
        ],
    };

    assert_second_operation_blocked(&spec, &plan);
}

#[test]
fn scheduler_allows_parallel_artifacts_with_different_build_inputs() {
    let mut spec = ResolvedBuildSpec::new("parallel-resource-test");
    spec.sources = vec![path_source("workspace", "workspace")];
    spec.artifacts = vec![
        node_artifact("node-a", "workspace", "packages/app-a", "out/a.tgz"),
        node_artifact("node-b", "workspace", "packages/app-b", "out/b.tgz"),
    ];
    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![
            parallel_artifact_operation(
                OperationId::artifact(&spec.artifacts[0].id),
                OperationKind::BuildArtifact {
                    artifact_id: spec.artifacts[0].id.clone(),
                },
            ),
            parallel_artifact_operation(
                OperationId::artifact(&spec.artifacts[1].id),
                OperationKind::BuildArtifact {
                    artifact_id: spec.artifacts[1].id.clone(),
                },
            ),
        ],
    };

    assert_eq!(
        next_schedulable_operation(&spec, &plan, &[0, 0], &[false, false], &[true, false]),
        Some(1)
    );
}

#[test]
fn scheduler_blocks_parallel_installs_with_same_destination() {
    let mut spec = ResolvedBuildSpec::new("parallel-resource-test");
    spec.install.entries = vec![
        InstallEntrySpec {
            id: InstallId::new("install-a"),
            artifact: ArtifactRef::new(ArtifactId::new("artifact-a")),
            dest: "/usr/bin/app".into(),
            replace: true,
            mode: None,
            owner: None,
            group: None,
        },
        InstallEntrySpec {
            id: InstallId::new("install-b"),
            artifact: ArtifactRef::new(ArtifactId::new("artifact-b")),
            dest: "/usr/bin/app".into(),
            replace: true,
            mode: None,
            owner: None,
            group: None,
        },
    ];
    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![
            parallel_runtime_operation(
                OperationId::install(&spec.install.entries[0].id),
                OperationKind::InstallArtifact {
                    install_id: spec.install.entries[0].id.clone(),
                    artifact: spec.install.entries[0].artifact.clone(),
                },
            ),
            parallel_runtime_operation(
                OperationId::install(&spec.install.entries[1].id),
                OperationKind::InstallArtifact {
                    install_id: spec.install.entries[1].id.clone(),
                    artifact: spec.install.entries[1].artifact.clone(),
                },
            ),
        ],
    };

    assert_second_operation_blocked(&spec, &plan);
}

#[test]
fn scheduler_blocks_parallel_stage_services_with_same_unit_path() {
    let mut spec = ResolvedBuildSpec::new("parallel-resource-test");
    spec.stage.services = vec![
        StageServiceSpec {
            id: StageItemId::new("svc-a"),
            name: "a".into(),
            unit_path: "/etc/systemd/system/app.service".into(),
        },
        StageServiceSpec {
            id: StageItemId::new("svc-b"),
            name: "b".into(),
            unit_path: "/etc/systemd/system/app.service".into(),
        },
    ];
    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![
            parallel_runtime_operation(
                OperationId::stage_service(&spec.stage.services[0].id),
                OperationKind::RenderStageService {
                    item_id: spec.stage.services[0].id.clone(),
                },
            ),
            parallel_runtime_operation(
                OperationId::stage_service(&spec.stage.services[1].id),
                OperationKind::RenderStageService {
                    item_id: spec.stage.services[1].id.clone(),
                },
            ),
        ],
    };

    assert_second_operation_blocked(&spec, &plan);
}

#[test]
fn scheduler_blocks_parallel_images_with_same_collect_dir() {
    let mut spec = ResolvedBuildSpec::new("parallel-resource-test");
    spec.image = ImageSpec::new(ImageDefinition::Buildroot(BuildrootImageSpec::default()));
    spec.image.output.collect_dir = Some("out/images".into());
    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![
            parallel_operation(
                OperationId::image_prepare(),
                OperationKind::PrepareImage,
                OperationParallelismDomain::Images,
            ),
            parallel_operation(
                OperationId::image(),
                OperationKind::BuildImage,
                OperationParallelismDomain::Images,
            ),
        ],
    };

    assert_second_operation_blocked(&spec, &plan);
}

#[test]
fn scheduler_blocks_parallel_checkpoints_with_same_id() {
    let spec = ResolvedBuildSpec::new("parallel-resource-test");
    let checkpoint_id = CheckpointId::new("base-image");
    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![
            parallel_operation(
                OperationId::checkpoint(&checkpoint_id),
                OperationKind::CaptureCheckpoint {
                    checkpoint_id: checkpoint_id.clone(),
                },
                OperationParallelismDomain::Checkpoints,
            ),
            parallel_operation(
                OperationId::new("checkpoint-base-image-copy"),
                OperationKind::CaptureCheckpoint { checkpoint_id },
                OperationParallelismDomain::Checkpoints,
            ),
        ],
    };

    assert_second_operation_blocked(&spec, &plan);
}
