mod command;
mod contract;
mod digest;
mod outputs;
#[cfg(test)]
mod tests;

pub use command::{
    command_for_execution, command_output_with_timeout, command_output_with_timeout_and_sink,
    run_command_with_retries,
};
pub use contract::{
    ArtifactDependencyContract, ArtifactDockerExecution, ArtifactExecutionBackend,
    ArtifactExecutionContract, ArtifactOutputContract, ArtifactOutputKind,
};
pub use digest::{
    ArtifactBackendState, command_version_line, dir_digest, file_sha256_or_placeholder, path_bytes,
    produced_filename, render_artifact_backend_state,
};
pub use gaia_process::{ProcessCancelCheck, ProcessLogLine, ProcessLogSink, sleep_with_cancel};
pub use outputs::{
    artifact_marker_contract, artifact_output_path, artifact_package_root, artifact_sidecar_path,
    artifact_state_path, copy_artifact_file_to_output, ensure_artifact_output_parent,
    finalize_temp_output, materialize_artifact_marker_and_state, materialize_artifact_output,
    materialize_artifact_state, render_build_context_state,
};

use gaia_spec::{ArtifactProviderKind, ArtifactSpec, ResolvedBuildSpec, RetryBackoffStrategySpec};
use std::time::Duration;

pub trait ArtifactProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn kind(&self) -> ArtifactProviderKind;
    fn supports(&self, _spec: &ResolvedBuildSpec) -> bool {
        true
    }
    fn plan_artifact(&self, artifact: &ArtifactSpec) -> ArtifactPlan {
        ArtifactPlan {
            operations: vec![ArtifactProviderOperation::Build],
            contract: ArtifactExecutionContract::from_spec(
                artifact,
                None,
                false,
                ArtifactExecutionContract::default_command_policy(),
                gaia_spec::OutputRetentionPolicySpec::default(),
            ),
        }
    }
    fn validate_artifact(&self, _artifact: &ArtifactSpec) -> Vec<ArtifactProviderValidationIssue> {
        Vec::new()
    }
    fn execute_artifact(
        &self,
        artifact: &ArtifactSpec,
        _contract: &ArtifactExecutionContract,
        _log_sink: Option<ProcessLogSink>,
        _cancel_check: Option<ProcessCancelCheck>,
    ) -> Result<Vec<String>, ArtifactProviderError> {
        Err(ArtifactProviderError::new(
            ArtifactProviderErrorKind::PolicyBlocked,
            format!(
                "artifact provider '{}' must implement execute_artifact for '{}'",
                self.id(),
                artifact.id.as_str(),
            ),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactPlan {
    pub operations: Vec<ArtifactProviderOperation>,
    pub contract: ArtifactExecutionContract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactProviderOperation {
    Build,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactProviderValidationIssue {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactProviderErrorKind {
    ToolStart,
    Timeout,
    Cancelled,
    OutputMissing,
    BackendCommand,
    PolicyBlocked,
    RuntimeState,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactProviderError {
    pub kind: ArtifactProviderErrorKind,
    pub message: String,
}

impl ArtifactProviderError {
    pub fn new(kind: ArtifactProviderErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn backend_command(message: impl Into<String>) -> Self {
        Self::new(ArtifactProviderErrorKind::BackendCommand, message)
    }

    pub fn runtime_state(message: impl Into<String>) -> Self {
        Self::new(ArtifactProviderErrorKind::RuntimeState, message)
    }
}

impl From<String> for ArtifactProviderError {
    fn from(value: String) -> Self {
        Self::new(ArtifactProviderErrorKind::Unknown, value)
    }
}

pub fn retry_backoff_duration(
    strategy: RetryBackoffStrategySpec,
    base_backoff_ms: u64,
    attempt: u32,
) -> Duration {
    gaia_process::retry_backoff_duration(
        match strategy {
            RetryBackoffStrategySpec::Fixed => gaia_process::ProcessRetryBackoffStrategy::Fixed,
            RetryBackoffStrategySpec::Exponential => {
                gaia_process::ProcessRetryBackoffStrategy::Exponential
            }
        },
        base_backoff_ms,
        attempt,
    )
}

#[derive(Default)]
pub struct ArtifactProviderCatalog {
    providers: Vec<Box<dyn ArtifactProvider>>,
}

impl ArtifactProviderCatalog {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    pub fn register(&mut self, provider: Box<dyn ArtifactProvider>) {
        self.providers.push(provider);
    }

    pub fn find_for_kind(&self, kind: ArtifactProviderKind) -> Option<&dyn ArtifactProvider> {
        self.providers
            .iter()
            .map(Box::as_ref)
            .find(|provider| provider.kind() == kind)
    }
}
