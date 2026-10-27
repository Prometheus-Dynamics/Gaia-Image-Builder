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
            features: Vec::new(),
            no_default_features: false,
            all_features: false,
            build_group: None,
            group_packages: Vec::new(),
            cargo_git_sources: Vec::new(),
            build_env: Vec::new(),
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

/// The operation the scheduler picks while operation `running` runs, with
/// nothing else started.
fn pick_beside(spec: &ResolvedBuildSpec, plan: &ExecutionPlan, running: usize) -> Option<usize> {
    let resources = OperationResources::new(spec, plan);
    let count = plan.operations.len();
    let mut running_flags = vec![false; count];
    running_flags[running] = true;
    let mut ledger = RunningLedger::default();
    ledger.start(&resources, running);
    next_schedulable_operation(
        plan,
        &resources,
        &vec![0; count],
        &vec![false; count],
        &running_flags,
        &mut ledger,
    )
}

fn assert_second_operation_blocked(spec: &ResolvedBuildSpec, plan: &ExecutionPlan) {
    assert_eq!(pick_beside(spec, plan, 0), None);
}

/// What a simulated run did: the operations started together in each wave
/// (a wave ends when an operation finishes), and the order of all starts.
struct SimulatedRun {
    waves: Vec<Vec<usize>>,
    started: Vec<usize>,
}

/// Runs `plan` to the end with the scheduler's picking rules. Operations
/// finish in the order they started. Every start is checked against the
/// running operations: an exclusive operation never runs beside another, and
/// no two running operations hold the same resource key.
fn simulate(spec: &ResolvedBuildSpec, plan: &ExecutionPlan) -> SimulatedRun {
    let resources = OperationResources::new(spec, plan);
    let count = plan.operations.len();
    let mut remaining = vec![0usize; count];
    for (index, operation) in plan.operations.iter().enumerate() {
        remaining[index] = operation.depends_on.len();
    }
    let mut completed = vec![false; count];
    let mut running = vec![false; count];
    let mut ledger = RunningLedger::default();
    let mut in_flight: Vec<usize> = Vec::new();
    let mut run = SimulatedRun {
        waves: Vec::new(),
        started: Vec::new(),
    };
    let mut wave = Vec::new();
    loop {
        while let Some(index) = next_schedulable_operation(
            plan,
            &resources,
            &remaining,
            &completed,
            &running,
            &mut ledger,
        ) {
            if resources.is_exclusive(index) {
                assert!(
                    in_flight.is_empty(),
                    "exclusive operation {index} started beside {in_flight:?}"
                );
            }
            for other in &in_flight {
                assert!(
                    !resources.is_exclusive(*other),
                    "operation {index} started beside exclusive operation {other}"
                );
                assert!(
                    !resources
                        .keys(index)
                        .iter()
                        .any(|key| resources.keys(*other).contains(key)),
                    "operations {index} and {other} hold the same resource"
                );
            }
            running[index] = true;
            ledger.start(&resources, index);
            in_flight.push(index);
            wave.push(index);
            run.started.push(index);
        }
        if in_flight.is_empty() {
            break;
        }
        if !wave.is_empty() {
            run.waves.push(std::mem::take(&mut wave));
        }
        let done = in_flight.remove(0);
        running[done] = false;
        completed[done] = true;
        ledger.finish(&resources, done);
        let done_id = plan.operations[done].id.clone();
        for (index, operation) in plan.operations.iter().enumerate() {
            if operation.depends_on.contains(&done_id) {
                remaining[index] -= 1;
            }
        }
    }
    assert!(
        completed.iter().all(|done| *done),
        "scheduler stalled with operations not started: {:?}",
        completed
            .iter()
            .enumerate()
            .filter(|(_, done)| !**done)
            .map(|(index, _)| index)
            .collect::<Vec<_>>()
    );
    run
}

fn stage_spec(files: &[(&str, &str)]) -> ResolvedBuildSpec {
    let mut spec = ResolvedBuildSpec::new("parallel-resource-test");
    spec.stage.files = files
        .iter()
        .map(|(id, dest)| StageFileSpec {
            id: StageItemId::new(*id),
            src: (*id).into(),
            content: None,
            dest: (*dest).into(),
            mode: None,
            origin: StageContentOriginSpec::StaticAsset,
        })
        .collect();
    spec
}

fn stage_operation(spec: &ResolvedBuildSpec, index: usize) -> PlannedOperation {
    let item = spec.stage.files[index].id.clone();
    parallel_runtime_operation(
        OperationId::stage_file(&item),
        OperationKind::RenderStageFile { item_id: item },
    )
}

#[test]
fn idle_scheduler_overlaps_parallel_work_before_an_exclusive_operation() {
    // The exclusive operation comes first in the plan. The old picker started
    // it alone when nothing ran, so the two parallel operations waited for it.
    let spec = stage_spec(&[
        ("exclusive", "/etc/exclusive"),
        ("a", "/etc/a"),
        ("b", "/etc/b"),
    ]);
    let exclusive_id = OperationId::stage_file(&spec.stage.files[0].id);
    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations: vec![
            PlannedOperation::new(
                exclusive_id,
                OperationKind::RenderStageFile {
                    item_id: spec.stage.files[0].id.clone(),
                },
            )
            .with_reuse(OperationReuse::execute("test", "test operation")),
            stage_operation(&spec, 1),
            stage_operation(&spec, 2),
        ],
    };

    let run = simulate(&spec, &plan);

    assert_eq!(run.waves[0], vec![1, 2], "parallel work starts together");
    assert_eq!(
        run.waves.last(),
        Some(&vec![0]),
        "exclusive runs alone last"
    );
}

#[test]
fn conflicting_and_exclusive_operations_never_overlap() {
    // Two pairs of stage files share a destination, one operation depends on
    // another, and an exclusive operation sits in the middle of the plan.
    let spec = stage_spec(&[
        ("x1", "/etc/x"),
        ("x2", "/etc/x"),
        ("y1", "/etc/y"),
        ("y2", "/etc/y"),
        ("z", "/etc/z"),
        ("w", "/etc/w"),
    ]);
    let mut operations: Vec<PlannedOperation> =
        (0..6).map(|index| stage_operation(&spec, index)).collect();
    operations[3] = PlannedOperation::new(
        OperationId::stage_file(&spec.stage.files[3].id),
        OperationKind::RenderStageFile {
            item_id: spec.stage.files[3].id.clone(),
        },
    )
    .with_reuse(OperationReuse::execute("test", "test operation"));
    operations[5].depends_on = vec![OperationId::stage_file(&spec.stage.files[0].id)];
    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations,
    };

    let run = simulate(&spec, &plan);

    assert_eq!(run.started.len(), 6);
    assert!(
        run.waves.iter().any(|wave| wave.len() >= 2),
        "some work overlapped: {:?}",
        run.waves
    );
}

#[test]
fn exclusive_operation_is_not_starved_by_a_stream_of_parallel_work() {
    // One exclusive operation is ready at the start. The parallel operations
    // form a chain, so exactly one of them is ready at a time, and a picker
    // without the bypass limit would run all of them first.
    const CHAIN: usize = 12;
    let files: Vec<(String, String)> =
        std::iter::once(("exclusive".to_string(), "/etc/exclusive".to_string()))
            .chain(
                (0..CHAIN).map(|index| (format!("chain-{index}"), format!("/etc/chain-{index}"))),
            )
            .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(id, dest)| (id.as_str(), dest.as_str()))
        .collect();
    let spec = stage_spec(&refs);
    let mut operations = vec![
        PlannedOperation::new(
            OperationId::stage_file(&spec.stage.files[0].id),
            OperationKind::RenderStageFile {
                item_id: spec.stage.files[0].id.clone(),
            },
        )
        .with_reuse(OperationReuse::execute("test", "test operation")),
    ];
    for index in 1..=CHAIN {
        let mut operation = stage_operation(&spec, index);
        if index > 1 {
            operation.depends_on = vec![OperationId::stage_file(&spec.stage.files[index - 1].id)];
        }
        operations.push(operation);
    }
    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations,
    };

    let run = simulate(&spec, &plan);

    let exclusive_at = run
        .started
        .iter()
        .position(|index| *index == 0)
        .expect("exclusive operation started");
    assert!(
        exclusive_at <= 8,
        "exclusive operation started at position {exclusive_at} of {}",
        run.started.len()
    );
}

#[test]
fn scheduler_blocks_parallel_stage_files_with_same_destination() {
    let mut spec = ResolvedBuildSpec::new("parallel-resource-test");
    spec.stage.files = vec![
        StageFileSpec {
            id: StageItemId::new("motd-a"),
            src: "a".into(),
            content: None,
            dest: "/etc/motd".into(),
            mode: None,
            origin: StageContentOriginSpec::StaticAsset,
        },
        StageFileSpec {
            id: StageItemId::new("motd-b"),
            src: "b".into(),
            content: None,
            dest: "/etc/motd".into(),
            mode: None,
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
            content: None,
            dest: "/etc/motd".into(),
            mode: None,
            origin: StageContentOriginSpec::StaticAsset,
        },
        StageFileSpec {
            id: StageItemId::new("issue"),
            src: "b".into(),
            content: None,
            dest: "/etc/issue".into(),
            mode: None,
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

    assert_eq!(pick_beside(&spec, &plan, 0), Some(1));
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

    assert_eq!(pick_beside(&spec, &plan, 0), Some(1));
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
