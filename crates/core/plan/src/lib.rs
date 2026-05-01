mod graph;
mod operations;
mod reuse;

pub use graph::{ExecutionPlan, PlanDiagnostic, ReuseState};
pub use operations::{
    OperationId, OperationKind, OperationOptionality, OperationParallelism,
    OperationParallelismDomain, OperationParallelismMode, OperationReuse, PlannedOperation,
    RebuildReason,
};
pub use reuse::{operation_output_signature, spec_fingerprint};

use gaia_artifact_providers::{ArtifactProviderCatalog, ArtifactProviderOperation};
use gaia_image_providers::{ImageProviderCatalog, ImageProviderOperation};
use gaia_source_providers::{SourceProviderCatalog, SourceProviderOperation};
use gaia_spec::{ImageDefinition, ResolvedBuildSpec};

use crate::reuse::{
    apply_reuse_state, artifact_rebuild_message, checkpoint_anchor_dependency,
    checkpoint_optionality, operation_fingerprint,
};

pub fn plan_build(
    spec: &ResolvedBuildSpec,
    source_catalog: &SourceProviderCatalog,
    artifact_catalog: &ArtifactProviderCatalog,
    image_catalog: &ImageProviderCatalog,
) -> ExecutionPlan {
    plan_build_with_reuse_state(spec, source_catalog, artifact_catalog, image_catalog, None)
}

pub fn plan_build_with_reuse_state(
    spec: &ResolvedBuildSpec,
    source_catalog: &SourceProviderCatalog,
    artifact_catalog: &ArtifactProviderCatalog,
    image_catalog: &ImageProviderCatalog,
    reuse_state: Option<&ReuseState>,
) -> ExecutionPlan {
    let span = tracing::info_span!(
        "plan_build",
        build_id = %spec.identity.id.as_str(),
        build_name = %spec.identity.build_name,
        sources = spec.sources.len(),
        artifacts = spec.artifacts.len(),
        reuse_state = reuse_state.is_some(),
    );
    let _guard = span.enter();
    let resolve = PlannedOperation::new(OperationId::resolve(), OperationKind::ResolveBuild)
        .with_parallelism(OperationParallelism::exclusive(
            OperationParallelismDomain::Global,
        ))
        .with_optionality(OperationOptionality::Required)
        .with_fingerprint(operation_fingerprint(spec, &OperationKind::ResolveBuild))
        .with_reuse(OperationReuse::execute(
            "plan_resolution_required",
            "build resolution always executes for a fresh plan",
        ));
    let mut operations = vec![resolve];

    for source in &spec.sources {
        let provider = source_catalog
            .find_for_kind(source.provider_kind())
            .expect("source provider must be registered before planning");
        for operation in provider.plan_source(source) {
            let planned = match operation {
                SourceProviderOperation::Materialize => PlannedOperation::new(
                    OperationId::source(&source.id),
                    OperationKind::MaterializeSource {
                        source_id: source.id.clone(),
                    },
                )
                .with_parallelism(OperationParallelism::parallelizable(
                    OperationParallelismDomain::Sources,
                ))
                .with_optionality(OperationOptionality::Required)
                .with_fingerprint(operation_fingerprint(
                    spec,
                    &OperationKind::MaterializeSource {
                        source_id: source.id.clone(),
                    },
                ))
                .with_dependency(OperationId::resolve())
                .with_reuse(OperationReuse::execute(
                    "source_materialization_required",
                    format!(
                        "source '{}' will materialize because no reuse state exists yet",
                        source.id.as_str()
                    ),
                )),
            };
            operations.push(planned);
        }
    }

    for artifact in &spec.artifacts {
        let provider = artifact_catalog
            .find_for_kind(artifact.provider_kind())
            .expect("artifact provider must be registered before planning");
        let artifact_plan = provider.plan_artifact(artifact);
        for operation in artifact_plan.operations {
            let mut planned = match operation {
                ArtifactProviderOperation::Build => PlannedOperation::new(
                    OperationId::artifact(&artifact.id),
                    OperationKind::BuildArtifact {
                        artifact_id: artifact.id.clone(),
                    },
                )
                .with_parallelism(OperationParallelism::parallelizable(
                    OperationParallelismDomain::Artifacts,
                ))
                .with_optionality(OperationOptionality::Required)
                .with_fingerprint(operation_fingerprint(
                    spec,
                    &OperationKind::BuildArtifact {
                        artifact_id: artifact.id.clone(),
                    },
                ))
                .with_dependency(OperationId::resolve())
                .with_reuse(OperationReuse::execute(
                    "artifact_build_required",
                    artifact_rebuild_message(artifact),
                )),
            };
            if let Some(source) = &artifact.source {
                planned = planned.with_dependency(OperationId::source(&source.id));
            }
            for dependency in &artifact.dependencies {
                planned = planned.with_dependency(OperationId::artifact(&dependency.id));
            }
            operations.push(planned);
        }
    }

    for install in &spec.install.entries {
        operations.push(
            PlannedOperation::new(
                OperationId::install(&install.id),
                OperationKind::InstallArtifact {
                    install_id: install.id.clone(),
                    artifact: install.artifact.clone(),
                },
            )
            .with_parallelism(OperationParallelism::parallelizable(
                OperationParallelismDomain::Runtime,
            ))
            .with_optionality(OperationOptionality::Required)
            .with_fingerprint(operation_fingerprint(
                spec,
                &OperationKind::InstallArtifact {
                    install_id: install.id.clone(),
                    artifact: install.artifact.clone(),
                },
            ))
            .with_dependency(OperationId::artifact(&install.artifact.id))
            .with_reuse(OperationReuse::execute(
                "install_depends_on_artifact",
                format!(
                    "install '{}' will execute because artifact '{}' is part of this plan",
                    install.id.as_str(),
                    install.artifact.id.as_str()
                ),
            )),
        );
    }

    let stage_dependencies = spec
        .install
        .entries
        .iter()
        .map(|install| OperationId::install(&install.id))
        .collect::<Vec<_>>();
    let mut stage_operation_ids = Vec::new();
    for file in &spec.stage.files {
        let op_id = OperationId::stage_file(&file.id);
        stage_operation_ids.push(op_id.clone());
        operations.push(PlannedOperation {
            id: op_id,
            kind: OperationKind::RenderStageFile {
                item_id: file.id.clone(),
            },
            depends_on: stage_dependencies.clone(),
            parallelism: OperationParallelism::parallelizable(OperationParallelismDomain::Runtime),
            optionality: OperationOptionality::Required,
            fingerprint: operation_fingerprint(
                spec,
                &OperationKind::RenderStageFile {
                    item_id: file.id.clone(),
                },
            ),
            reuse: OperationReuse::execute(
                "stage_file_required",
                format!(
                    "stage file '{}' will render from staged inputs",
                    file.id.as_str()
                ),
            ),
        });
    }
    for env_set in &spec.stage.env_sets {
        let op_id = OperationId::stage_env_set(&env_set.id);
        stage_operation_ids.push(op_id.clone());
        operations.push(PlannedOperation {
            id: op_id,
            kind: OperationKind::RenderStageEnvSet {
                item_id: env_set.id.clone(),
            },
            depends_on: stage_dependencies.clone(),
            parallelism: OperationParallelism::parallelizable(OperationParallelismDomain::Runtime),
            optionality: OperationOptionality::Required,
            fingerprint: operation_fingerprint(
                spec,
                &OperationKind::RenderStageEnvSet {
                    item_id: env_set.id.clone(),
                },
            ),
            reuse: OperationReuse::execute(
                "stage_env_required",
                format!(
                    "stage env set '{}' will render from staged inputs",
                    env_set.id.as_str()
                ),
            ),
        });
    }
    for service in &spec.stage.services {
        let op_id = OperationId::stage_service(&service.id);
        stage_operation_ids.push(op_id.clone());
        operations.push(PlannedOperation {
            id: op_id,
            kind: OperationKind::RenderStageService {
                item_id: service.id.clone(),
            },
            depends_on: stage_dependencies.clone(),
            parallelism: OperationParallelism::parallelizable(OperationParallelismDomain::Runtime),
            optionality: OperationOptionality::Required,
            fingerprint: operation_fingerprint(
                spec,
                &OperationKind::RenderStageService {
                    item_id: service.id.clone(),
                },
            ),
            reuse: OperationReuse::execute(
                "stage_service_required",
                format!(
                    "stage service '{}' will render from staged inputs",
                    service.id.as_str()
                ),
            ),
        });
    }

    let provider = image_catalog
        .find_for_kind(spec.image.provider_kind())
        .expect("image provider must be registered before planning");
    let image_plan = provider.plan_image(&spec.image);
    let mut image_prepare_dependencies = vec![OperationId::resolve()];
    let mut image_finalize_dependencies =
        if stage_operation_ids.is_empty() && stage_dependencies.is_empty() {
            vec![OperationId::resolve()]
        } else {
            let mut deps = stage_dependencies.clone();
            deps.extend(stage_operation_ids.clone());
            deps
        };
    match &spec.image.definition {
        ImageDefinition::Buildroot(buildroot) => {
            if let Some(source_id) = &buildroot.source {
                image_prepare_dependencies.push(OperationId::source(source_id));
                image_finalize_dependencies.push(OperationId::source(source_id));
            }
        }
        ImageDefinition::StartingPoint(starting_point) => {
            if let Some(source_id) = &starting_point.source {
                image_prepare_dependencies.push(OperationId::source(source_id));
                image_finalize_dependencies.push(OperationId::source(source_id));
            }
        }
    }
    image_prepare_dependencies.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    image_prepare_dependencies.dedup_by(|left, right| left.as_str() == right.as_str());
    image_finalize_dependencies.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    image_finalize_dependencies.dedup_by(|left, right| left.as_str() == right.as_str());
    let has_image_prepare = image_plan
        .operations
        .contains(&ImageProviderOperation::Prepare);
    for operation in image_plan.operations {
        let planned = match operation {
            ImageProviderOperation::Prepare => {
                let mut planned = PlannedOperation::new(
                    OperationId::image_prepare(),
                    OperationKind::PrepareImage,
                );
                planned = planned.with_parallelism(OperationParallelism::parallelizable(
                    OperationParallelismDomain::Images,
                ));
                planned = planned.with_optionality(OperationOptionality::Required);
                planned = planned
                    .with_fingerprint(operation_fingerprint(spec, &OperationKind::PrepareImage));
                for dependency in &image_prepare_dependencies {
                    planned = planned.with_dependency(dependency.clone());
                }
                planned.with_reuse(OperationReuse::execute(
                    "image_prepare_required",
                    "image base preparation will execute before final image assembly",
                ))
            }
            ImageProviderOperation::Build => {
                let mut planned =
                    PlannedOperation::new(OperationId::image(), OperationKind::BuildImage);
                planned = planned.with_parallelism(OperationParallelism::parallelizable(
                    OperationParallelismDomain::Images,
                ));
                planned = planned.with_optionality(OperationOptionality::Required);
                planned = planned
                    .with_fingerprint(operation_fingerprint(spec, &OperationKind::BuildImage));
                if has_image_prepare {
                    planned = planned.with_dependency(OperationId::image_prepare());
                }
                for dependency in &image_finalize_dependencies {
                    planned = planned.with_dependency(dependency.clone());
                }
                planned.with_reuse(OperationReuse::execute(
                    "image_build_required",
                    "image build will execute because staged inputs are part of this plan",
                ))
            }
        };
        operations.push(planned);
    }

    for checkpoint in &spec.checkpoints.points {
        let anchor_dependency = checkpoint_anchor_dependency(&checkpoint.anchor);
        let checkpoint_optionality = checkpoint_optionality(checkpoint);
        operations.push(
            PlannedOperation::new(
                OperationId::checkpoint(&checkpoint.id),
                OperationKind::CaptureCheckpoint {
                    checkpoint_id: checkpoint.id.clone(),
                },
            )
            .with_parallelism(OperationParallelism::parallelizable(
                OperationParallelismDomain::Checkpoints,
            ))
            .with_optionality(checkpoint_optionality)
            .with_fingerprint(operation_fingerprint(
                spec,
                &OperationKind::CaptureCheckpoint {
                    checkpoint_id: checkpoint.id.clone(),
                },
            ))
            .with_dependency(anchor_dependency.clone())
            .with_reuse(OperationReuse::execute(
                "checkpoint_capture_required",
                format!(
                    "checkpoint '{}' will capture after '{}'",
                    checkpoint.id.as_str(),
                    anchor_dependency.as_str()
                ),
            )),
        );
    }

    let mut report = PlannedOperation::new(OperationId::report(), OperationKind::EmitReport)
        .with_parallelism(OperationParallelism::exclusive(
            OperationParallelismDomain::Reporting,
        ))
        .with_optionality(OperationOptionality::Required)
        .with_fingerprint(operation_fingerprint(spec, &OperationKind::EmitReport))
        .with_dependency(OperationId::image())
        .with_reuse(OperationReuse::execute(
            "report_emission_required",
            "report emission always runs at the end of a plan",
        ));
    for checkpoint in &spec.checkpoints.points {
        if checkpoint_optionality(checkpoint) != OperationOptionality::BestEffort {
            report = report.with_dependency(OperationId::checkpoint(&checkpoint.id));
        }
    }
    operations.push(report);

    let plan = ExecutionPlan {
        build_id: spec.identity.id.clone(),
        operations,
    };
    let plan = apply_reuse_state(plan, spec, reuse_state);
    tracing::debug!(
        operations = plan.operations.len(),
        diagnostics = plan.validate().len(),
        "planned build operations"
    );
    debug_assert!(plan.validate().is_empty(), "generated plan must be valid");
    plan
}
