//! Artifact build dispatch, including batched builds of artifacts that one
//! provider invocation can produce together.

use super::*;
use gaia_artifact_providers::{
    ArtifactBatchItem, ArtifactProvider, ArtifactProviderError, ArtifactProviderErrorKind,
};
use gaia_spec::{ArtifactId, ArtifactSpec};

struct PreparedArtifact<'env> {
    artifact: &'env ArtifactSpec,
    provider: &'env dyn ArtifactProvider,
    contract: ArtifactExecutionContract,
}

fn prepare_artifact<'env>(
    operation_id: &OperationId,
    artifact_id: &ArtifactId,
    spec: &'env ResolvedBuildSpec,
    providers: &ExecutionProviders<'env>,
    job_budget: Option<usize>,
) -> Result<PreparedArtifact<'env>, Box<OperationExecutionResult>> {
    let Some(artifact) = spec
        .artifacts
        .iter()
        .find(|artifact| artifact.id == *artifact_id)
    else {
        return Err(Box::new(failure_with_kind(
            operation_id.clone(),
            "missing_artifact_spec",
            ExecutionErrorKind::MissingSpec,
            format!("missing artifact spec '{}'", artifact_id.as_str()),
        )));
    };
    let Some(provider) = providers
        .artifact_catalog
        .find_for_kind(artifact.provider_kind())
    else {
        return Err(Box::new(failure_with_kind(
            operation_id.clone(),
            "missing_artifact_provider",
            ExecutionErrorKind::MissingProvider,
            format!("missing artifact provider for '{}'", artifact_id.as_str()),
        )));
    };
    let artifact_execution_policy = spec
        .policy
        .providers
        .artifact_command_policy(artifact.provider_kind());
    let mut contract = ArtifactExecutionContract::from_spec(
        artifact,
        resolve_artifact_source_dir(spec, artifact),
        matches!(artifact.definition, ArtifactDefinition::Rust(_))
            && spec.policy.providers.rust.allow_nested_build,
        artifact_execution_policy,
        spec.policy.execution.output_retention,
    )
    .try_with_build_context(spec)
    .map_err(|message| {
        Box::new(failure_with_cleanup_and_tail(
            operation_id.clone(),
            "artifact_contract_invalid",
            execution_error_kind_from_artifact(&message.kind),
            message.message.clone(),
            output_tail(&[message.message], spec),
            RollbackDomain::Artifacts,
            Vec::new(),
        ))
    })?;
    contract.job_budget = job_budget;
    Ok(PreparedArtifact {
        artifact,
        provider,
        contract,
    })
}

/// Turns a provider result into the operation result, exactly as a single
/// build does.
fn artifact_result(
    operation_id: &OperationId,
    artifact_id: &ArtifactId,
    spec: &ResolvedBuildSpec,
    contract: &ArtifactExecutionContract,
    tail: &LogTail,
    result: Result<Vec<String>, ArtifactProviderError>,
) -> OperationExecutionResult {
    match result {
        Ok(messages) => success_from_messages(
            operation_id.clone(),
            messages,
            format!("built artifact '{}'", artifact_id.as_str()),
            RollbackDomain::Artifacts,
            artifact_cleanup_paths(contract),
        ),
        Err(message) if matches!(message.kind, ArtifactProviderErrorKind::Cancelled) => {
            cancelled_with_cleanup(
                operation_id.clone(),
                message.message,
                RollbackDomain::Artifacts,
                artifact_cleanup_paths(contract),
            )
        }
        Err(message) => failure_with_cleanup_and_tail(
            operation_id.clone(),
            "artifact_execution_failed",
            execution_error_kind_from_artifact(&message.kind),
            message.message.clone(),
            tail.failure_tail(&message.message, spec),
            RollbackDomain::Artifacts,
            artifact_cleanup_paths(contract),
        ),
    }
}

pub(crate) fn execute_artifact_operation(
    operation_id: &OperationId,
    artifact_id: &ArtifactId,
    spec: &ResolvedBuildSpec,
    providers: &ExecutionProviders<'_>,
    context: &DispatchContext,
) -> OperationExecutionResult {
    let prepared = match prepare_artifact(
        operation_id,
        artifact_id,
        spec,
        providers,
        context.job_budget,
    ) {
        Ok(prepared) => prepared,
        Err(failure) => return *failure,
    };
    let tail = LogTail::for_spec(spec);
    let log_sink = tail.sink(operation_id.clone(), context.event_sender.clone());
    let result = prepared.provider.execute_artifact(
        prepared.artifact,
        &prepared.contract,
        log_sink,
        context.cancel_check.clone(),
    );
    artifact_result(
        operation_id,
        artifact_id,
        spec,
        &prepared.contract,
        &tail,
        result,
    )
}

/// Key under which `operation` may share one provider invocation with other
/// artifact builds, or `None` when it must build alone.
pub(crate) fn artifact_batch_key(
    operation: &PlannedOperation,
    spec: &ResolvedBuildSpec,
    providers: &ExecutionProviders<'_>,
) -> Option<String> {
    let OperationKind::BuildArtifact { artifact_id } = &operation.kind else {
        return None;
    };
    if !operation.reuse.should_execute() {
        return None;
    }
    let prepared = prepare_artifact(&operation.id, artifact_id, spec, providers, None).ok()?;
    if prepared.artifact.provider_kind() == gaia_spec::ArtifactProviderKind::Rust
        && !spec.policy.providers.rust.batch_builds
    {
        return None;
    }
    prepared
        .provider
        .batch_key(prepared.artifact, &prepared.contract)
        .map(|key| format!("{}|{key}", prepared.provider.id()))
}

/// Builds several artifact operations that share a batch key with one
/// provider invocation. Returns one result per operation, in order.
pub(crate) fn dispatch_artifact_batch(
    operations: &[PlannedOperation],
    spec: &ResolvedBuildSpec,
    providers: &ExecutionProviders<'_>,
    context: &DispatchContext,
) -> Vec<OperationExecutionResult> {
    let mut results = operations.iter().map(|_| None).collect::<Vec<_>>();
    let mut members = Vec::new();
    for (position, operation) in operations.iter().enumerate() {
        let OperationKind::BuildArtifact { artifact_id } = &operation.kind else {
            results[position] = Some(dispatch_operation(operation, spec, providers, context));
            continue;
        };
        match prepare_artifact(
            &operation.id,
            artifact_id,
            spec,
            providers,
            context.job_budget,
        ) {
            Ok(prepared) => members.push((position, artifact_id, prepared)),
            Err(failure) => results[position] = Some(*failure),
        }
    }
    if let Some((_, _, leader)) = members.first() {
        let provider = leader.provider;
        let tails = members
            .iter()
            .map(|_| LogTail::for_spec(spec))
            .collect::<Vec<_>>();
        let items = members
            .iter()
            .zip(&tails)
            .map(|((position, _, prepared), tail)| ArtifactBatchItem {
                artifact: prepared.artifact,
                contract: &prepared.contract,
                log_sink: tail.sink(
                    operations[*position].id.clone(),
                    context.event_sender.clone(),
                ),
            })
            .collect::<Vec<_>>();
        tracing::info!(
            operations = ?members
                .iter()
                .map(|(position, _, _)| operations[*position].id.as_str())
                .collect::<Vec<_>>(),
            provider = provider.id(),
            "building artifacts in one batched invocation"
        );
        let provider_results =
            provider.execute_artifact_batch(&items, context.cancel_check.clone());
        drop(items);
        for (((position, artifact_id, prepared), tail), result) in
            members.iter().zip(&tails).zip(provider_results)
        {
            results[*position] = Some(artifact_result(
                &operations[*position].id,
                artifact_id,
                spec,
                &prepared.contract,
                tail,
                result,
            ));
        }
    }
    results
        .into_iter()
        .zip(operations)
        .map(|(result, operation)| {
            result.unwrap_or_else(|| {
                failure_with_kind(
                    operation.id.clone(),
                    "artifact_batch_missing_result",
                    ExecutionErrorKind::Unknown,
                    format!(
                        "artifact provider returned no result for '{}' in a batched build",
                        operation.id.as_str()
                    ),
                )
            })
        })
        .collect()
}
