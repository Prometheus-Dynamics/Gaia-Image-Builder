mod artifact;
mod assembly;
mod helpers;
pub use helpers::image_execution_policy;

pub(crate) use artifact::{artifact_batch_key, dispatch_artifact_batch};

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

/// Per-dispatch inputs shared by every operation kind.
#[derive(Clone)]
pub(crate) struct DispatchContext {
    pub(crate) build_name: String,
    pub(crate) event_sender: Option<mpsc::Sender<ExecutionEvent>>,
    pub(crate) cancel_check: Option<gaia_process::ProcessCancelCheck>,
    /// CPU budget for the spawned build tool when several CPU-heavy
    /// operations run at once; `None` keeps the tool defaults.
    pub(crate) job_budget: Option<usize>,
}

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
    /// Not run because an operation it depends on failed. Only emitted with
    /// `policy.failure.keep_going`, where independent work continues.
    Skipped {
        operation_id: OperationId,
        reason: String,
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

    /// A failure whose error kind is `Cancelled` was stopped by the stop or
    /// cancel signal (provider and process errors of that kind), not by a
    /// fault of its own. It becomes a cancelled result, so its partial outputs
    /// are cleaned under the same rules and it is neither recorded as
    /// completed nor counted as a failure.
    pub(crate) fn into_cancelled_if_stopped(mut self) -> Self {
        let Some(error) = self.error.as_ref() else {
            return self;
        };
        if error.kind != ExecutionErrorKind::Cancelled {
            return self;
        }
        let message = error.message.clone();
        self.error = None;
        self.cancelled = true;
        self.events
            .retain(|event| !matches!(event, ExecutionEvent::Failed { .. }));
        self.events.push(ExecutionEvent::Log {
            operation_id: self.operation_id.clone(),
            message,
        });
        self
    }

    fn with_cleanup_domain(mut self, cleanup_domain: RollbackDomain) -> Self {
        self.cleanup_domain = Some(cleanup_domain);
        self
    }

    /// Adds log messages ahead of the result's own events.
    fn with_log_messages(mut self, messages: Vec<String>) -> Self {
        let logs = messages.into_iter().map(|message| ExecutionEvent::Log {
            operation_id: self.operation_id.clone(),
            message,
        });
        self.events.splice(0..0, logs.collect::<Vec<_>>());
        self
    }
}

pub(crate) fn dispatch_operation(
    operation: &PlannedOperation,
    spec: &ResolvedBuildSpec,
    providers: &ExecutionProviders<'_>,
    context: &DispatchContext,
) -> OperationExecutionResult {
    let build_name = context.build_name.as_str();
    let event_sender = context.event_sender.clone();
    let cancel_check = context.cancel_check.clone();
    let span = tracing::info_span!(
        "execute_operation",
        build_id = %spec.identity.id.as_str(),
        operation_id = %operation.id.as_str(),
        operation_kind = ?operation.kind,
        parallelism_mode = ?operation.parallelism.mode,
        parallelism_domain = ?operation.parallelism.domain,
        reused = matches!(operation.reuse, OperationReuse::Reuse { .. }),
        job_budget = ?context.job_budget,
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
                let tail = LogTail::for_spec(spec);
                let log_sink = tail.sink(operation.id.clone(), event_sender.clone());
                success_from_messages(
                    operation.id.clone(),
                    match provider.execute_source(spec, source, log_sink, cancel_check.clone()) {
                        Ok(messages) => messages,
                        Err(message) => {
                            if matches!(
                                message.kind,
                                gaia_source_providers::SourceProviderErrorKind::Cancelled
                            ) {
                                return cancelled_with_cleanup(
                                    operation.id.clone(),
                                    message.message,
                                    RollbackDomain::Sources,
                                    source_cleanup_paths(spec, source),
                                );
                            }
                            return failure_with_cleanup_and_tail(
                                operation.id.clone(),
                                "source_execution_failed",
                                execution_error_kind_from_source(&message.kind),
                                message.message.clone(),
                                tail.failure_tail(&message.message, spec),
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
            OperationKind::BuildArtifact { artifact_id } => artifact::execute_artifact_operation(
                &operation.id,
                artifact_id,
                spec,
                providers,
                context,
            ),
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
                let tail = LogTail::for_spec(spec);
                let log_sink = tail.sink(operation.id.clone(), event_sender.clone());
                let mut image_policy = image_execution_policy(spec);
                if let Some(jobs) = context.job_budget
                    && image_policy.local_jobs == 0
                {
                    image_policy.local_jobs = u32::try_from(jobs).unwrap_or(u32::MAX);
                }
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
                    Ok(result) => result,
                    Err(message) => {
                        let result = if matches!(
                            message.kind,
                            gaia_image_providers::ImageProviderErrorKind::Cancelled
                        ) {
                            cancelled_with_cleanup(
                                operation.id.clone(),
                                message.message.clone(),
                                RollbackDomain::Images,
                                image_definition_cleanup_paths(spec),
                            )
                        } else {
                            failure_with_cleanup_and_tail(
                                operation.id.clone(),
                                "image_execution_failed",
                                execution_error_kind_from_image(&message.kind),
                                message.message.clone(),
                                tail.failure_tail(&message.message, spec),
                                RollbackDomain::Images,
                                image_definition_cleanup_paths(spec),
                            )
                        };
                        // Where the time went before the failure.
                        return result.with_log_messages(message.step_times);
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
                            warnings: Vec::new(),
                            notes: Vec::new(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_spec::RollbackDomain;

    #[test]
    fn cancelled_kind_failure_becomes_cancelled_result_with_cleanup() {
        let result = helpers::failure_with_cleanup(
            OperationId::new("source:slow"),
            "stopped",
            ExecutionErrorKind::Cancelled,
            "slow cancelled".into(),
            RollbackDomain::Sources,
            vec![PathBuf::from("build/sources/slow")],
        )
        .into_cancelled_if_stopped();

        assert!(result.error.is_none(), "{:?}", result.error);
        assert!(result.cancelled);
        assert_eq!(result.cleanup_domain, Some(RollbackDomain::Sources));
        assert_eq!(
            result.cleanup_paths,
            vec![PathBuf::from("build/sources/slow")]
        );
        assert!(
            !result
                .events
                .iter()
                .any(|event| matches!(event, ExecutionEvent::Failed { .. }))
        );
    }

    #[test]
    fn other_failure_kinds_stay_failures() {
        let result = helpers::failure_with_kind(
            OperationId::new("source:fail"),
            "backend",
            ExecutionErrorKind::BackendCommand,
            "boom".into(),
        )
        .into_cancelled_if_stopped();

        assert!(!result.cancelled);
        assert!(result.error.is_some());
    }
}
