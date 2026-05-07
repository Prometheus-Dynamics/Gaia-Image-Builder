mod fs;
mod operations;
mod process;
mod runtime;
mod scheduler;

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;

use gaia_artifact_providers::ArtifactProviderCatalog;
use gaia_image_providers::ImageProviderCatalog;
use gaia_plan::{ExecutionPlan, OperationId};
use gaia_process::ProcessCancelCheck;
use gaia_source_providers::SourceProviderCatalog;
use gaia_spec::ResolvedBuildSpec;
use runtime::ExecutionRuntime;
use scheduler::{
    ScheduleReadyContext, ScheduleReadyState, next_pending_operation_id, resolve_parallel_jobs,
    schedule_ready_operations,
};

pub use operations::{
    ExecutionCleanupStatus, ExecutionError, ExecutionErrorKind, ExecutionEvent,
    OperationExecutionResult,
};
pub use runtime::{CleanupFailure, ExecutionCancellation, ExecutionContext, ExecutionOutcome};

pub struct ExecutionProviders<'a> {
    pub source_catalog: &'a SourceProviderCatalog,
    pub artifact_catalog: &'a ArtifactProviderCatalog,
    pub image_catalog: &'a ImageProviderCatalog,
}

pub fn execute_plan(
    spec: &ResolvedBuildSpec,
    plan: &ExecutionPlan,
    providers: ExecutionProviders<'_>,
) -> ExecutionOutcome {
    execute_plan_with_cancellation_and_observer(
        spec,
        plan,
        providers,
        &ExecutionCancellation::new(),
        None,
    )
}

pub fn execute_plan_with_cancellation(
    spec: &ResolvedBuildSpec,
    plan: &ExecutionPlan,
    providers: ExecutionProviders<'_>,
    cancellation: &ExecutionCancellation,
) -> ExecutionOutcome {
    execute_plan_with_cancellation_and_observer(spec, plan, providers, cancellation, None)
}

pub fn execute_plan_with_cancellation_and_observer(
    spec: &ResolvedBuildSpec,
    plan: &ExecutionPlan,
    providers: ExecutionProviders<'_>,
    cancellation: &ExecutionCancellation,
    event_sender: Option<Sender<ExecutionEvent>>,
) -> ExecutionOutcome {
    let max_parallel_jobs = resolve_parallel_jobs(spec);
    let span = tracing::info_span!(
        "execute_plan",
        build_id = %spec.identity.id.as_str(),
        build_name = %spec.identity.build_name,
        operations = plan.operations.len(),
        max_parallel_jobs,
        rollback_on_error = spec.policy.failure.rollback_on_error,
    );
    let _guard = span.enter();
    let context = ExecutionContext::new(spec);
    let mut runtime = ExecutionRuntime::new(context, event_sender);
    let build_name = runtime.context().build_name.clone();
    let observer = runtime.event_sender();
    let operation_count = plan.operations.len();
    let operation_index: HashMap<&str, usize> = plan
        .operations
        .iter()
        .enumerate()
        .map(|(index, operation)| (operation.id.as_str(), index))
        .collect();
    let mut remaining_dependencies = vec![0usize; operation_count];
    let mut dependents = vec![Vec::<usize>::new(); operation_count];
    for (index, operation) in plan.operations.iter().enumerate() {
        remaining_dependencies[index] = operation.depends_on.len();
        for dependency in &operation.depends_on {
            if let Some(&dependency_index) = operation_index.get(dependency.as_str()) {
                dependents[dependency_index].push(index);
            }
        }
    }
    let mut completed = vec![false; operation_count];
    let mut running = vec![false; operation_count];
    let mut running_count = 0usize;
    let mut first_failure: Option<(
        OperationId,
        Option<gaia_spec::RollbackDomain>,
        Vec<std::path::PathBuf>,
    )> = None;
    let mut cancellation_pending = false;
    let mut cancelled_cleanup: Option<(
        OperationId,
        Option<gaia_spec::RollbackDomain>,
        Vec<std::path::PathBuf>,
    )> = None;
    let stop_running_operations = Arc::new(AtomicBool::new(false));
    let cancel_check: ProcessCancelCheck = {
        let cancellation = cancellation.clone();
        let stop_running_operations = stop_running_operations.clone();
        Arc::new(move || {
            cancellation.is_cancelled() || stop_running_operations.load(Ordering::SeqCst)
        })
    };

    let schedule_context = ScheduleReadyContext {
        spec,
        plan,
        providers: &providers,
        build_name: build_name.as_str(),
        event_sender: observer.clone(),
        cancel_check: cancel_check.clone(),
        max_parallel_jobs,
    };

    thread::scope(|scope| {
        let (result_tx, result_rx) =
            std::sync::mpsc::channel::<(usize, OperationExecutionResult)>();
        loop {
            if cancellation.is_cancelled() {
                cancellation_pending = true;
            }

            if !cancellation_pending && first_failure.is_none() {
                let scheduled_any = schedule_ready_operations(
                    scope,
                    &result_tx,
                    &mut runtime,
                    &schedule_context,
                    ScheduleReadyState {
                        remaining_dependencies: &remaining_dependencies,
                        completed: &completed,
                        running: &mut running,
                        running_count: &mut running_count,
                    },
                );
                if scheduled_any {
                    continue;
                }
            }

            if running_count == 0 {
                if let Some((failed_operation_id, failed_cleanup_domain, failed_cleanup_paths)) =
                    first_failure.take()
                {
                    if spec.policy.failure.rollback_on_error {
                        runtime.rollback(
                            &failed_operation_id,
                            failed_cleanup_domain,
                            &failed_cleanup_paths,
                            spec.policy.failure.preserve_failed_outputs,
                            &spec.policy.failure.rollback_domains,
                        );
                    }
                } else if cancellation_pending {
                    let cancelled_operation_id = cancelled_cleanup
                        .as_ref()
                        .map(|(operation_id, _, _)| operation_id.clone())
                        .or_else(|| next_pending_operation_id(plan, &completed, &running))
                        .unwrap_or_else(|| {
                            plan.operations
                                .first()
                                .map(|operation| operation.id.clone())
                                .unwrap_or_else(OperationId::resolve)
                        });
                    let (cancelled_cleanup_domain, cancelled_cleanup_paths) = cancelled_cleanup
                        .take()
                        .map(|(_, domain, paths)| (domain, paths))
                        .unwrap_or((None, Vec::new()));
                    runtime.cancel(
                        &cancelled_operation_id,
                        spec.policy.failure.rollback_on_error,
                        &spec.policy.failure.rollback_domains,
                        cancelled_cleanup_domain,
                        &cancelled_cleanup_paths,
                    );
                }
                break;
            }

            let Ok((index, result)) = result_rx.recv() else {
                break;
            };
            running[index] = false;
            running_count = running_count.saturating_sub(1);

            if result.cancelled {
                tracing::warn!(
                    operation_id = %result.operation_id.as_str(),
                    cleanup_domain = ?result.cleanup_domain,
                    cleanup_paths = result.cleanup_paths.len(),
                    "operation cancelled"
                );
                cancellation_pending = true;
                cancelled_cleanup = Some((
                    result.operation_id.clone(),
                    result.cleanup_domain,
                    result.cleanup_paths.clone(),
                ));
                runtime.record(result);
                continue;
            }

            let succeeded = result.error.is_none();
            if succeeded {
                if let Some(source) = &result.reused_source {
                    tracing::info!(
                        operation_id = %result.operation_id.as_str(),
                        reused_from = %source,
                        "operation reused"
                    );
                } else {
                    tracing::info!(
                        operation_id = %result.operation_id.as_str(),
                        "operation succeeded"
                    );
                }
                completed[index] = true;
                for &dependent in &dependents[index] {
                    remaining_dependencies[dependent] =
                        remaining_dependencies[dependent].saturating_sub(1);
                }
            } else if first_failure.is_none() {
                if let Some(error) = &result.error {
                    tracing::warn!(
                        operation_id = %result.operation_id.as_str(),
                        error_code = error.code,
                        error_kind = ?error.kind,
                        output_tail_lines = error.output_tail.len(),
                        cleanup_domain = ?result.cleanup_domain,
                        cleanup_paths = result.cleanup_paths.len(),
                        "operation failed"
                    );
                } else {
                    tracing::warn!(
                        operation_id = %result.operation_id.as_str(),
                        cleanup_domain = ?result.cleanup_domain,
                        cleanup_paths = result.cleanup_paths.len(),
                        "operation failed without error detail"
                    );
                }
                first_failure = Some((
                    result.operation_id.clone(),
                    result.cleanup_domain,
                    result.cleanup_paths.clone(),
                ));
                stop_running_operations.store(true, Ordering::SeqCst);
            }

            runtime.record(result);
        }
    });

    runtime.finish()
}
