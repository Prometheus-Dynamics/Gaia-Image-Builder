use gaia_image_providers::ImageExecutionResult;
use gaia_plan::OperationId;
use gaia_process::{ProcessLogLine, ProcessLogSink};
use gaia_spec::{ResolvedBuildSpec, RollbackDomain};
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::{ExecutionCleanupStatus, ExecutionError, ExecutionEvent, OperationExecutionResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionContext {
    pub build_name: String,
}

impl ExecutionContext {
    pub fn new(spec: &ResolvedBuildSpec) -> Self {
        Self {
            build_name: spec.identity.display_name.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutionOutcome {
    pub completed_operations: usize,
    pub completed_ids: Vec<OperationId>,
    pub reused_ids: Vec<OperationId>,
    pub rolled_back_ids: Vec<OperationId>,
    pub cancelled: bool,
    pub cancelled_operation_id: Option<OperationId>,
    pub image_results: Vec<ImageExecutionResult>,
    pub events: Vec<ExecutionEvent>,
    pub errors: Vec<ExecutionError>,
    pub cleanup_failures: Vec<CleanupFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupFailure {
    pub operation_id: OperationId,
    pub path: PathBuf,
    pub message: String,
}

#[derive(Clone, Default)]
pub struct ExecutionCancellation {
    cancelled: Arc<AtomicBool>,
}

impl ExecutionCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

pub struct ExecutionRuntime {
    context: ExecutionContext,
    outcome: ExecutionOutcome,
    cleanup_stack: Vec<(OperationId, Option<RollbackDomain>, Vec<PathBuf>)>,
    event_sender: Option<Sender<ExecutionEvent>>,
}

impl ExecutionRuntime {
    pub fn new(context: ExecutionContext, event_sender: Option<Sender<ExecutionEvent>>) -> Self {
        Self {
            context,
            outcome: ExecutionOutcome::default(),
            cleanup_stack: Vec::new(),
            event_sender,
        }
    }

    pub fn context(&self) -> &ExecutionContext {
        &self.context
    }

    pub fn record(&mut self, result: OperationExecutionResult) {
        let cleanup_paths = result.cleanup_paths.clone();
        let cleanup_domain = result.cleanup_domain;
        self.outcome
            .image_results
            .extend(result.image_results.clone());
        for event in result.events {
            self.emit_event(event);
        }
        if let Some(error) = result.error {
            self.outcome.errors.push(error);
            return;
        }
        if result.cancelled {
            return;
        }
        if let Some(source) = result.reused_source {
            self.outcome.completed_operations += 1;
            self.outcome.completed_ids.push(result.operation_id.clone());
            self.outcome.reused_ids.push(result.operation_id.clone());
            let _ = source;
            return;
        }
        self.outcome.completed_operations += 1;
        self.cleanup_stack
            .push((result.operation_id.clone(), cleanup_domain, cleanup_paths));
        self.outcome.completed_ids.push(result.operation_id);
    }

    pub fn emit_event(&mut self, event: ExecutionEvent) {
        if let Some(sender) = &self.event_sender {
            let _ = sender.send(event.clone());
        }
        self.outcome.events.push(event);
    }

    pub fn event_sender(&self) -> Option<Sender<ExecutionEvent>> {
        self.event_sender.clone()
    }

    pub fn rollback(
        &mut self,
        failed_operation_id: &OperationId,
        failed_cleanup_domain: Option<RollbackDomain>,
        failed_cleanup_paths: &[PathBuf],
        preserve_failed_outputs: bool,
        rollback_domains: &[RollbackDomain],
    ) {
        if !preserve_failed_outputs
            && !failed_cleanup_paths.is_empty()
            && cleanup_domain_enabled(failed_cleanup_domain, rollback_domains)
        {
            let failures = cleanup_paths(failed_operation_id, failed_cleanup_paths);
            let status = if failures.is_empty() {
                ExecutionCleanupStatus::Cleaned
            } else {
                ExecutionCleanupStatus::Failed
            };
            self.record_error_cleanup(failed_operation_id, status, &failures);
            self.outcome.cleanup_failures.extend(failures.clone());
            self.emit_event(ExecutionEvent::Log {
                operation_id: failed_operation_id.clone(),
                message: cleanup_message("cleaned", failed_cleanup_paths.len(), failures.len()),
            });
        } else if preserve_failed_outputs && !failed_cleanup_paths.is_empty() {
            self.record_error_cleanup(failed_operation_id, ExecutionCleanupStatus::Preserved, &[]);
            self.emit_event(ExecutionEvent::Log {
                operation_id: failed_operation_id.clone(),
                message: format!(
                    "preserved {} failed output path(s) for debugging",
                    failed_cleanup_paths.len()
                ),
            });
        } else if !failed_cleanup_paths.is_empty() {
            self.record_error_cleanup(
                failed_operation_id,
                ExecutionCleanupStatus::DomainDisabled,
                &[],
            );
            self.emit_event(ExecutionEvent::Log {
                operation_id: failed_operation_id.clone(),
                message: format!(
                    "kept {} failed output path(s) because rollback domain is disabled",
                    failed_cleanup_paths.len()
                ),
            });
        }
        while let Some((operation_id, cleanup_domain, cleanup_paths_for_op)) =
            self.cleanup_stack.pop()
        {
            if cleanup_domain_enabled(cleanup_domain, rollback_domains) {
                let failures = cleanup_paths(&operation_id, &cleanup_paths_for_op);
                self.outcome.cleanup_failures.extend(failures.clone());
                self.outcome.rolled_back_ids.push(operation_id.clone());
                self.emit_event(ExecutionEvent::Log {
                    operation_id,
                    message: cleanup_message(
                        "rolled back",
                        cleanup_paths_for_op.len(),
                        failures.len(),
                    ),
                });
            } else {
                self.emit_event(ExecutionEvent::Log {
                    operation_id,
                    message: format!(
                        "kept {} output path(s) because rollback domain is disabled",
                        cleanup_paths_for_op.len()
                    ),
                });
            }
        }
        self.outcome.completed_operations = 0;
        self.outcome.completed_ids.clear();
        self.outcome.image_results.clear();
    }

    pub fn cancel(
        &mut self,
        operation_id: &OperationId,
        rollback_on_error: bool,
        rollback_domains: &[RollbackDomain],
        cancelled_cleanup_domain: Option<RollbackDomain>,
        cancelled_cleanup_paths: &[PathBuf],
    ) {
        self.outcome.cancelled = true;
        self.outcome.cancelled_operation_id = Some(operation_id.clone());
        self.emit_event(ExecutionEvent::Cancelled {
            operation_id: operation_id.clone(),
        });
        self.emit_event(ExecutionEvent::Log {
            operation_id: operation_id.clone(),
            message: "execution cancelled".into(),
        });
        if rollback_on_error {
            if !cancelled_cleanup_paths.is_empty()
                && cleanup_domain_enabled(cancelled_cleanup_domain, rollback_domains)
            {
                let failures = cleanup_paths(operation_id, cancelled_cleanup_paths);
                self.outcome.cleanup_failures.extend(failures.clone());
                self.emit_event(ExecutionEvent::Log {
                    operation_id: operation_id.clone(),
                    message: format!(
                        "{} from cancelled operation",
                        cleanup_message("cleaned", cancelled_cleanup_paths.len(), failures.len())
                    ),
                });
            }
            while let Some((completed_operation_id, cleanup_domain, cleanup_paths_for_op)) =
                self.cleanup_stack.pop()
            {
                if cleanup_domain_enabled(cleanup_domain, rollback_domains) {
                    let failures = cleanup_paths(&completed_operation_id, &cleanup_paths_for_op);
                    self.outcome.cleanup_failures.extend(failures.clone());
                    self.outcome
                        .rolled_back_ids
                        .push(completed_operation_id.clone());
                    self.emit_event(ExecutionEvent::Log {
                        operation_id: completed_operation_id,
                        message: format!(
                            "{} after cancellation",
                            cleanup_message(
                                "rolled back",
                                cleanup_paths_for_op.len(),
                                failures.len(),
                            )
                        ),
                    });
                }
            }
            self.outcome.completed_operations = 0;
            self.outcome.completed_ids.clear();
            self.outcome.image_results.clear();
        }
    }

    pub fn finish(self) -> ExecutionOutcome {
        self.outcome
    }

    fn record_error_cleanup(
        &mut self,
        operation_id: &OperationId,
        status: ExecutionCleanupStatus,
        failures: &[CleanupFailure],
    ) {
        if let Some(error) = self
            .outcome
            .errors
            .iter_mut()
            .find(|error| error.operation_id == *operation_id)
        {
            error.cleanup_status = status;
            error.cleanup_failures = failures
                .iter()
                .map(|failure| failure.message.clone())
                .collect();
        }
    }
}

pub fn process_log_sink(
    operation_id: OperationId,
    sender: Option<Sender<ExecutionEvent>>,
) -> Option<ProcessLogSink> {
    sender.map(|sender| {
        std::sync::Arc::new(move |line: ProcessLogLine| {
            let _ = sender.send(ExecutionEvent::Log {
                operation_id: operation_id.clone(),
                message: line.line,
            });
        }) as ProcessLogSink
    })
}

fn cleanup_paths(operation_id: &OperationId, paths: &[PathBuf]) -> Vec<CleanupFailure> {
    let mut failures = Vec::new();
    for path in paths {
        let result = if path.is_dir() {
            fs::remove_dir_all(path)
        } else if path.exists() {
            fs::remove_file(path)
        } else {
            Ok(())
        };
        if let Err(error) = result {
            failures.push(CleanupFailure {
                operation_id: operation_id.clone(),
                path: path.clone(),
                message: format!("failed to remove '{}': {error}", path.display()),
            });
        }
    }
    failures
}

fn cleanup_message(action: &str, attempted: usize, failures: usize) -> String {
    if failures == 0 {
        format!("{action} {attempted} output path(s)")
    } else {
        format!("{action} {attempted} output path(s), {failures} cleanup failure(s)")
    }
}

fn cleanup_domain_enabled(
    cleanup_domain: Option<RollbackDomain>,
    rollback_domains: &[RollbackDomain],
) -> bool {
    cleanup_domain.is_some_and(|domain| rollback_domains.contains(&domain))
}
