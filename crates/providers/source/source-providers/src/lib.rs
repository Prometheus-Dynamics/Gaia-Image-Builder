use gaia_process::{
    DockerRunSpec, ProcessOutputRetention, ProcessRetryBackoffStrategy, ProcessRunErrorKind,
    docker_run_command, label_process_log_sink,
    retry_backoff_duration as process_retry_backoff_duration,
    run_command_with_timeout_and_retention, sleep_with_cancel,
};
pub use gaia_process::{ProcessCancelCheck, ProcessLogLine, ProcessLogSink};
use gaia_spec::{
    ArchiveSourceSpec, GitSourceSpec, ResolvedBuildSpec, RetryBackoffStrategySpec,
    SourceDefinition, SourcePinPolicySpec, SourceProviderKind, SourceRefreshPolicySpec, SourceSpec,
    WorkspaceSpec,
};
#[cfg(test)]
use gaia_spec::{DownloadSourceSpec, PathSourceSpec};
use std::collections::hash_map::DefaultHasher;
#[cfg(test)]
use std::ffi::OsStr;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

pub trait SourceProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn kind(&self) -> SourceProviderKind;
    fn supports(&self, _spec: &ResolvedBuildSpec) -> bool {
        true
    }
    fn plan_source(&self, _source: &SourceSpec) -> Vec<SourceProviderOperation> {
        vec![SourceProviderOperation::Materialize]
    }
    fn validate_source(&self, _source: &SourceSpec) -> Vec<SourceProviderValidationIssue> {
        Vec::new()
    }
    fn execute_source(
        &self,
        _spec: &ResolvedBuildSpec,
        source: &SourceSpec,
        _log_sink: Option<ProcessLogSink>,
        _cancel_check: Option<ProcessCancelCheck>,
    ) -> Result<Vec<String>, SourceProviderError> {
        Err(SourceProviderError::new(
            SourceProviderErrorKind::PolicyBlocked,
            format!(
                "source provider '{}' must implement execute_source for '{}'",
                self.id(),
                source.id.as_str()
            ),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceProviderOperation {
    Materialize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceProviderValidationIssue {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceProviderErrorKind {
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
pub struct SourceProviderError {
    pub kind: SourceProviderErrorKind,
    pub message: String,
}

impl SourceProviderError {
    pub fn new(kind: SourceProviderErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn backend_command(message: impl Into<String>) -> Self {
        Self::new(SourceProviderErrorKind::BackendCommand, message)
    }

    pub fn output_missing(message: impl Into<String>) -> Self {
        Self::new(SourceProviderErrorKind::OutputMissing, message)
    }

    pub fn runtime_state(message: impl Into<String>) -> Self {
        Self::new(SourceProviderErrorKind::RuntimeState, message)
    }
}

impl From<String> for SourceProviderError {
    fn from(value: String) -> Self {
        Self::new(SourceProviderErrorKind::Unknown, value)
    }
}

#[derive(Default)]
pub struct SourceProviderCatalog {
    providers: Vec<Box<dyn SourceProvider>>,
}

impl SourceProviderCatalog {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    pub fn register(&mut self, provider: Box<dyn SourceProvider>) {
        self.providers.push(provider);
    }

    pub fn with_defaults() -> Self {
        let mut catalog = Self::new();
        catalog.register(Box::new(GitSourceProvider));
        catalog.register(Box::new(PathSourceProvider));
        catalog.register(Box::new(ArchiveSourceProvider));
        catalog.register(Box::new(DownloadSourceProvider));
        catalog
    }

    pub fn find_for_kind(&self, kind: SourceProviderKind) -> Option<&dyn SourceProvider> {
        self.providers
            .iter()
            .map(Box::as_ref)
            .find(|provider| provider.kind() == kind)
    }
}

pub struct GitSourceProvider;
pub struct PathSourceProvider;
pub struct ArchiveSourceProvider;
pub struct DownloadSourceProvider;

mod archive;
mod command;
mod digest;
mod download;
mod git;
mod git_helpers;
mod path;
mod state;
#[cfg(test)]
mod tests;

pub(crate) use command::*;
pub(crate) use digest::*;
pub(crate) use git_helpers::*;
pub(crate) use state::*;
