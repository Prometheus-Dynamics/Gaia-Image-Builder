mod assembly;
mod helpers;

use assembly::*;
use gaia_artifact_providers::ArtifactExecutionContract;
use gaia_plan::{OperationId, OperationKind, OperationReuse, PlannedOperation};
use gaia_spec::{ArtifactDefinition, ResolvedBuildSpec, RollbackDomain};
use helpers::*;
use std::path::PathBuf;

use crate::ExecutionProviders;
use crate::fs::FsMutation;
use crate::process;
use crate::runtime::process_log_sink;
use std::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionEvent {
    Started {
        operation_id: OperationId,
    },
    Log {
        operation_id: OperationId,
        message: String,
    },
    Succeeded {
        operation_id: OperationId,
    },
    Reused {
        operation_id: OperationId,
    },
    Cancelled {
        operation_id: OperationId,
    },
    Failed {
        operation_id: OperationId,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionError {
    pub code: &'static str,
    pub kind: ExecutionErrorKind,
    pub operation_id: OperationId,
    pub message: String,
    pub output_tail: Vec<String>,
    pub cleanup_domain: Option<RollbackDomain>,
    pub cleanup_paths: Vec<PathBuf>,
    pub cleanup_status: ExecutionCleanupStatus,
    pub cleanup_failures: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionErrorKind {
    MissingSpec,
    MissingProvider,
    ToolStart,
    Timeout,
    Cancelled,
    OutputMissing,
    BackendCommand,
    PolicyBlocked,
    RuntimeState,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionCleanupStatus {
    NotRequired,
    Cleaned,
    Preserved,
    DomainDisabled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationExecutionResult {
    pub operation_id: OperationId,
    pub events: Vec<ExecutionEvent>,
    pub error: Option<ExecutionError>,
    pub cancelled: bool,
    pub reused_source: Option<String>,
    pub image_results: Vec<gaia_image_providers::ImageExecutionResult>,
    pub cleanup_domain: Option<RollbackDomain>,
    pub cleanup_paths: Vec<PathBuf>,
}

impl OperationExecutionResult {
    pub fn success(operation_id: OperationId, message: String) -> Self {
        Self {
            events: vec![
                ExecutionEvent::Log {
                    operation_id: operation_id.clone(),
                    message,
                },
                ExecutionEvent::Succeeded {
                    operation_id: operation_id.clone(),
                },
            ],
            operation_id,
            error: None,
            cancelled: false,
            reused_source: None,
            image_results: Vec::new(),
            cleanup_domain: None,
            cleanup_paths: Vec::new(),
        }
    }

    fn with_cleanup_domain(mut self, cleanup_domain: RollbackDomain) -> Self {
        self.cleanup_domain = Some(cleanup_domain);
        self
    }
}

pub fn dispatch_operation(
    operation: &PlannedOperation,
    spec: &ResolvedBuildSpec,
    providers: &ExecutionProviders<'_>,
    build_name: &str,
    event_sender: Option<std::sync::mpsc::Sender<ExecutionEvent>>,
    cancel_check: Option<gaia_process::ProcessCancelCheck>,
) -> OperationExecutionResult {
    let span = tracing::info_span!(
        "execute_operation",
        build_id = %spec.identity.id.as_str(),
        operation_id = %operation.id.as_str(),
        operation_kind = ?operation.kind,
        parallelism_mode = ?operation.parallelism.mode,
        parallelism_domain = ?operation.parallelism.domain,
        reused = matches!(operation.reuse, OperationReuse::Reuse { .. }),
    );
    let _guard = span.enter();
    if let OperationReuse::Reuse { source } = &operation.reuse {
        OperationExecutionResult {
            operation_id: operation.id.clone(),
            events: vec![
                ExecutionEvent::Log {
                    operation_id: operation.id.clone(),
                    message: format!("reused from {source}"),
                },
                ExecutionEvent::Reused {
                    operation_id: operation.id.clone(),
                },
            ],
            error: None,
            cancelled: false,
            reused_source: Some(source.clone()),
            image_results: Vec::new(),
            cleanup_domain: None,
            cleanup_paths: Vec::new(),
        }
    } else {
        match &operation.kind {
            OperationKind::ResolveBuild => OperationExecutionResult::success(
                operation.id.clone(),
                format!("resolved build '{build_name}'"),
            ),
            OperationKind::MaterializeSource { source_id } => {
                let Some(source) = spec.sources.iter().find(|source| source.id == *source_id)
                else {
                    return failure_with_kind(
                        operation.id.clone(),
                        "missing_source_spec",
                        ExecutionErrorKind::MissingSpec,
                        format!("missing source spec '{}'", source_id.as_str()),
                    );
                };
                let Some(provider) = providers
                    .source_catalog
                    .find_for_kind(source.provider_kind())
                else {
                    return failure_with_kind(
                        operation.id.clone(),
                        "missing_source_provider",
                        ExecutionErrorKind::MissingProvider,
                        format!("missing source provider for '{}'", source_id.as_str()),
                    );
                };
                let (log_tx, log_rx) = mpsc::channel::<String>();
                let direct_sink = process_log_sink(operation.id.clone(), event_sender.clone());
                let log_sink = direct_sink.map(|direct_sink| {
                    std::sync::Arc::new(move |line: gaia_source_providers::ProcessLogLine| {
                        let _ = log_tx.send(line.line.clone());
                        direct_sink(line);
                    }) as gaia_source_providers::ProcessLogSink
                });
                success_from_messages(
                    operation.id.clone(),
                    match provider.execute_source(spec, source, log_sink, cancel_check.clone()) {
                        Ok(messages) => merge_streamed_logs(log_rx, messages),
                        Err(message) => {
                            let logs = merge_streamed_logs(log_rx, vec![message.message]);
                            if matches!(
                                message.kind,
                                gaia_source_providers::SourceProviderErrorKind::Cancelled
                            ) {
                                return cancelled_with_cleanup(
                                    operation.id.clone(),
                                    logs.join("\n"),
                                    RollbackDomain::Sources,
                                    source_cleanup_paths(spec, source),
                                );
                            }
                            return failure_with_cleanup_and_tail(
                                operation.id.clone(),
                                "source_execution_failed",
                                execution_error_kind_from_source(&message.kind),
                                logs.join("\n"),
                                output_tail(&logs, spec),
                                RollbackDomain::Sources,
                                source_cleanup_paths(spec, source),
                            );
                        }
                    },
                    format!("materialized source '{}'", source_id.as_str()),
                    RollbackDomain::Sources,
                    source_cleanup_paths(spec, source),
                )
            }
            OperationKind::BuildArtifact { artifact_id } => {
                let Some(artifact) = spec
                    .artifacts
                    .iter()
                    .find(|artifact| artifact.id == *artifact_id)
                else {
                    return failure_with_kind(
                        operation.id.clone(),
                        "missing_artifact_spec",
                        ExecutionErrorKind::MissingSpec,
                        format!("missing artifact spec '{}'", artifact_id.as_str()),
                    );
                };
                let Some(provider) = providers
                    .artifact_catalog
                    .find_for_kind(artifact.provider_kind())
                else {
                    return failure_with_kind(
                        operation.id.clone(),
                        "missing_artifact_provider",
                        ExecutionErrorKind::MissingProvider,
                        format!("missing artifact provider for '{}'", artifact_id.as_str()),
                    );
                };
                let artifact_execution_policy = spec
                    .policy
                    .providers
                    .artifact_command_policy(artifact.provider_kind());
                let contract = match ArtifactExecutionContract::from_spec(
                    artifact,
                    resolve_artifact_source_dir(spec, artifact),
                    matches!(artifact.definition, ArtifactDefinition::Rust(_))
                        && spec.policy.providers.rust.allow_nested_build,
                    artifact_execution_policy,
                    spec.policy.execution.output_retention,
                )
                .try_with_build_context(spec)
                {
                    Ok(contract) => contract,
                    Err(message) => {
                        return failure_with_cleanup_and_tail(
                            operation.id.clone(),
                            "artifact_contract_invalid",
                            execution_error_kind_from_artifact(&message.kind),
                            message.message.clone(),
                            output_tail(&[message.message], spec),
                            RollbackDomain::Artifacts,
                            Vec::new(),
                        );
                    }
                };
                let _ = process::ProcessSpec::new(format!("build:{}", artifact_id.as_str()));
                let (log_tx, log_rx) = mpsc::channel::<String>();
                let direct_sink = process_log_sink(operation.id.clone(), event_sender.clone());
                let log_sink = direct_sink.map(|direct_sink| {
                    std::sync::Arc::new(move |line: gaia_artifact_providers::ProcessLogLine| {
                        let _ = log_tx.send(line.line.clone());
                        direct_sink(line);
                    }) as gaia_artifact_providers::ProcessLogSink
                });
                success_from_messages(
                    operation.id.clone(),
                    match provider.execute_artifact(
                        artifact,
                        &contract,
                        log_sink,
                        cancel_check.clone(),
                    ) {
                        Ok(messages) => merge_streamed_logs(log_rx, messages),
                        Err(message) => {
                            let logs = merge_streamed_logs(log_rx, vec![message.message]);
                            if matches!(
                                message.kind,
                                gaia_artifact_providers::ArtifactProviderErrorKind::Cancelled
                            ) {
                                return cancelled_with_cleanup(
                                    operation.id.clone(),
                                    logs.join("\n"),
                                    RollbackDomain::Artifacts,
                                    artifact_cleanup_paths(&contract),
                                );
                            }
                            return failure_with_cleanup_and_tail(
                                operation.id.clone(),
                                "artifact_execution_failed",
                                execution_error_kind_from_artifact(&message.kind),
                                logs.join("\n"),
                                output_tail(&logs, spec),
                                RollbackDomain::Artifacts,
                                artifact_cleanup_paths(&contract),
                            );
                        }
                    },
                    format!("built artifact '{}'", artifact_id.as_str()),
                    RollbackDomain::Artifacts,
                    artifact_cleanup_paths(&contract),
                )
            }
            OperationKind::InstallArtifact {
                install_id,
                artifact,
            } => {
                let _ = FsMutation::install(format!(
                    "artifact:{} -> install:{}",
                    artifact.id.as_str(),
                    install_id.as_str()
                ));
                let install = spec
                    .install
                    .entries
                    .iter()
                    .find(|entry| entry.id == *install_id);
                let state = gaia_spec::KeyValueState::new()
                    .with("kind", "install")
                    .with("install_id", install_id.as_str())
                    .with("artifact_id", artifact.id.as_str())
                    .with(
                        "dest",
                        install.map(|entry| entry.dest.as_str()).unwrap_or_default(),
                    )
                    .with(
                        "replace",
                        install.map(|entry| entry.replace).unwrap_or(false),
                    )
                    .with(
                        "mode",
                        install
                            .and_then(|entry| entry.mode)
                            .map(|mode| format!("{mode:o}"))
                            .unwrap_or_default(),
                    )
                    .with(
                        "owner",
                        install
                            .and_then(|entry| entry.owner.as_deref())
                            .unwrap_or_default(),
                    )
                    .with(
                        "group",
                        install
                            .and_then(|entry| entry.group.as_deref())
                            .unwrap_or_default(),
                    );
                let state_path = install_state_path(spec, install_id);
                if let Err(message) = write_runtime_state(state_path.clone(), &state) {
                    return failure_with_cleanup(
                        operation.id.clone(),
                        "install_runtime_state_failed",
                        ExecutionErrorKind::RuntimeState,
                        message,
                        RollbackDomain::Installs,
                        vec![state_path],
                    );
                }
                OperationExecutionResult::success(
                    operation.id.clone(),
                    format!(
                        "installed artifact '{}' via '{}'",
                        artifact.id.as_str(),
                        install_id.as_str()
                    ),
                )
                .with_cleanup_domain(RollbackDomain::Installs)
                .with_cleanup_paths(vec![install_state_path(spec, install_id)])
            }
            OperationKind::RenderStageFile { item_id } => stage_result(
                spec,
                operation.id.clone(),
                StageRuntimeKind::File,
                "rendered stage file",
                item_id,
            ),
            OperationKind::RenderStageEnvSet { item_id } => stage_result(
                spec,
                operation.id.clone(),
                StageRuntimeKind::Env,
                "rendered stage env set",
                item_id,
            ),
            OperationKind::RenderStageService { item_id } => stage_result(
                spec,
                operation.id.clone(),
                StageRuntimeKind::Service,
                "rendered stage service",
                item_id,
            ),
            OperationKind::PrepareImage | OperationKind::BuildImage => {
                let provider_kind = spec.image.provider_kind();
                let Some(provider) = providers.image_catalog.find_for_kind(provider_kind) else {
                    return failure_with_kind(
                        operation.id.clone(),
                        "missing_image_provider",
                        ExecutionErrorKind::MissingProvider,
                        format!("missing image provider for '{provider_kind:?}'"),
                    );
                };
                let image_plan = provider.plan_image(&spec.image);
                let image_operation = match &operation.kind {
                    OperationKind::PrepareImage => {
                        gaia_image_providers::ImageProviderOperation::Prepare
                    }
                    OperationKind::BuildImage => {
                        gaia_image_providers::ImageProviderOperation::Build
                    }
                    _ => unreachable!(),
                };
                let _ = process::ProcessSpec::new("build-image");
                let (log_tx, log_rx) = mpsc::channel::<String>();
                let direct_sink = process_log_sink(operation.id.clone(), event_sender.clone());
                let log_sink = direct_sink.map(|direct_sink| {
                    std::sync::Arc::new(move |line: gaia_image_providers::ProcessLogLine| {
                        let _ = log_tx.send(line.line.clone());
                        direct_sink(line);
                    }) as gaia_image_providers::ProcessLogSink
                });
                let image_policy = image_execution_policy(spec);
                let image_result = match provider.execute_image_operation(
                    gaia_image_providers::ImageOperationExecution {
                        spec,
                        image: &spec.image,
                        operation: image_operation,
                        output: &image_plan.output,
                        policy: &image_policy,
                        log_sink,
                        cancel_check: cancel_check.clone(),
                    },
                ) {
                    Ok(mut result) => {
                        result.messages = merge_streamed_logs(log_rx, result.messages);
                        result
                    }
                    Err(message) => {
                        let logs = merge_streamed_logs(log_rx, vec![message.message]);
                        if matches!(
                            message.kind,
                            gaia_image_providers::ImageProviderErrorKind::Cancelled
                        ) {
                            return cancelled_with_cleanup(
                                operation.id.clone(),
                                logs.join("\n"),
                                RollbackDomain::Images,
                                image_definition_cleanup_paths(spec),
                            );
                        }
                        return failure_with_cleanup_and_tail(
                            operation.id.clone(),
                            "image_execution_failed",
                            execution_error_kind_from_image(&message.kind),
                            logs.join("\n"),
                            output_tail(&logs, spec),
                            RollbackDomain::Images,
                            image_definition_cleanup_paths(spec),
                        );
                    }
                };
                let image_cleanup = image_cleanup_paths(&image_result);
                success_from_messages(
                    operation.id.clone(),
                    image_result.messages.clone(),
                    match &operation.kind {
                        OperationKind::PrepareImage => "prepared image base".into(),
                        OperationKind::BuildImage => "built image".into(),
                        _ => unreachable!(),
                    },
                    RollbackDomain::Images,
                    image_cleanup,
                )
                .with_image_result(image_result)
            }
            OperationKind::AssembleImage => {
                let summary = match stage_image_assembly(spec, &operation.id, cancel_check.clone())
                {
                    Ok(summary) => summary,
                    Err(error) => {
                        return failure_with_cleanup_and_tail(
                            operation.id.clone(),
                            "assembly_execution_failed",
                            error.kind,
                            error.message.clone(),
                            output_tail(&[error.message], spec),
                            RollbackDomain::Images,
                            image_assembly_cleanup_paths(spec),
                        );
                    }
                };
                let state_path = assembly_state_path(spec);
                if let Err(message) = write_runtime_state(state_path.clone(), &summary.state) {
                    return failure_with_cleanup(
                        operation.id.clone(),
                        "assembly_runtime_state_failed",
                        ExecutionErrorKind::RuntimeState,
                        message,
                        RollbackDomain::Images,
                        image_assembly_cleanup_paths(spec),
                    );
                }
                let mut success = success_from_messages(
                    operation.id.clone(),
                    summary.messages,
                    "assembled image files".into(),
                    RollbackDomain::Images,
                    [summary.cleanup_paths, vec![assembly_state_path(spec)]].concat(),
                );
                if let Some(archive_path) = summary.archive_path {
                    success =
                        success.with_image_result(gaia_image_providers::ImageExecutionResult {
                            provider_id: format!("image.{}", spec.image.provider_kind().as_str()),
                            collect_dir: spec.image.output.collect_dir.as_ref().map(PathBuf::from),
                            archive_path: Some(archive_path),
                            emit_report: spec.image.output.emit_report,
                            reused: false,
                            reuse_details: Vec::new(),
                            messages: Vec::new(),
                            state_details: Vec::new(),
                        });
                }
                success
            }
            OperationKind::CaptureCheckpoint { checkpoint_id } => {
                let checkpoint = spec
                    .checkpoints
                    .points
                    .iter()
                    .find(|checkpoint| checkpoint.id == *checkpoint_id);
                let state = gaia_spec::KeyValueState::new()
                    .with("kind", "checkpoint")
                    .with("checkpoint_id", checkpoint_id.as_str())
                    .with(
                        "backend",
                        checkpoint
                            .and_then(|checkpoint| checkpoint.backend.as_ref())
                            .map(|backend| backend.backend.as_str())
                            .unwrap_or_default(),
                    )
                    .with(
                        "anchor",
                        checkpoint
                            .map(|checkpoint| checkpoint.anchor.as_str())
                            .unwrap_or_else(|| "image".to_string()),
                    )
                    .with(
                        "use_policy",
                        format!(
                            "{:?}",
                            checkpoint
                                .map(|checkpoint| checkpoint.use_policy)
                                .unwrap_or_default()
                        ),
                    )
                    .with(
                        "upload_policy",
                        format!(
                            "{:?}",
                            checkpoint
                                .map(|checkpoint| checkpoint.upload_policy)
                                .unwrap_or_default()
                        ),
                    );
                let state_path = checkpoint_state_path(spec, checkpoint_id);
                if let Err(message) = write_runtime_state(state_path.clone(), &state) {
                    return failure_with_cleanup(
                        operation.id.clone(),
                        "checkpoint_runtime_state_failed",
                        ExecutionErrorKind::RuntimeState,
                        message,
                        RollbackDomain::Checkpoints,
                        vec![state_path],
                    );
                }
                OperationExecutionResult::success(
                    operation.id.clone(),
                    format!("captured checkpoint '{}'", checkpoint_id.as_str()),
                )
                .with_cleanup_domain(RollbackDomain::Checkpoints)
                .with_cleanup_paths(vec![checkpoint_state_path(spec, checkpoint_id)])
            }
            OperationKind::EmitReport => {
                OperationExecutionResult::success(operation.id.clone(), "emitted report".into())
            }
        }
    }
}
