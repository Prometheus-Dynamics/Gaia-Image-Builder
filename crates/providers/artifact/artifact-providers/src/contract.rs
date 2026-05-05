use gaia_spec::{
    ArtifactDefinition, ArtifactExecutionSpec, ArtifactProviderKind, ArtifactRef, ArtifactSpec,
    ArtifactVariantSpec, BuildModeSpec, DockerExecutionSpec, OutputRetentionPolicySpec,
    ResolvedBuildSpec, ResolvedCommandPolicySpec, RetryBackoffStrategySpec, SourceRef,
};
use std::fs;
use std::path::{Path, PathBuf};

use crate::{ArtifactProviderError, ArtifactProviderErrorKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactExecutionContract {
    pub provider: ArtifactProviderKind,
    pub source: Option<SourceRef>,
    pub source_dir: Option<String>,
    pub workspace_root: Option<String>,
    pub execution_backend_explicit: bool,
    pub execution_backend: ArtifactExecutionBackend,
    pub artifact_target: Option<String>,
    pub build_version: Option<String>,
    pub build_branch: Option<String>,
    pub build_target: Option<String>,
    pub build_profile: Option<String>,
    pub allow_nested_build: bool,
    pub retry_attempts: u32,
    pub retry_backoff_ms: u64,
    pub retry_backoff_strategy: RetryBackoffStrategySpec,
    pub timeout_seconds: u64,
    pub output_retention: OutputRetentionPolicySpec,
    pub build_mode: Option<BuildModeSpec>,
    pub dependencies: Vec<ArtifactDependencyContract>,
    pub output: ArtifactOutputContract,
}

impl ArtifactExecutionContract {
    pub fn default_command_policy() -> ResolvedCommandPolicySpec {
        ResolvedCommandPolicySpec {
            retry_attempts: 1,
            retry_backoff_ms: 0,
            retry_backoff_strategy: RetryBackoffStrategySpec::Fixed,
            timeout_seconds: 300,
            local_jobs: 0,
            download_dir: None,
            ccache: Default::default(),
        }
    }

    pub fn from_spec(
        artifact: &ArtifactSpec,
        source_dir: Option<String>,
        allow_nested_build: bool,
        command_policy: ResolvedCommandPolicySpec,
        output_retention: OutputRetentionPolicySpec,
    ) -> Self {
        Self {
            provider: artifact.provider_kind(),
            source: artifact.source.clone(),
            source_dir,
            workspace_root: None,
            execution_backend_explicit: artifact.execution.is_some(),
            execution_backend: artifact
                .execution
                .as_ref()
                .map(|execution| match execution {
                    ArtifactExecutionSpec::Host => ArtifactExecutionBackend::Host,
                    ArtifactExecutionSpec::Docker(docker) => ArtifactExecutionBackend::Docker(
                        ArtifactDockerExecution::from_artifact(docker),
                    ),
                })
                .unwrap_or(ArtifactExecutionBackend::Host),
            artifact_target: artifact.target.clone(),
            build_version: None,
            build_branch: None,
            build_target: None,
            build_profile: None,
            allow_nested_build,
            retry_attempts: command_policy.retry_attempts,
            retry_backoff_ms: command_policy.retry_backoff_ms,
            retry_backoff_strategy: command_policy.retry_backoff_strategy,
            timeout_seconds: command_policy.timeout_seconds,
            output_retention,
            build_mode: artifact.build_mode.clone(),
            dependencies: artifact
                .dependencies
                .iter()
                .cloned()
                .map(ArtifactDependencyContract::from_ref)
                .collect(),
            output: ArtifactOutputContract::from_spec(artifact),
        }
    }

    pub fn with_build_context(mut self, spec: &ResolvedBuildSpec) -> Self {
        self.apply_build_context(spec);
        self
    }

    pub fn try_with_build_context(
        mut self,
        spec: &ResolvedBuildSpec,
    ) -> Result<Self, ArtifactProviderError> {
        self.apply_build_context(spec);
        self.validate_release_invariants()?;
        Ok(self)
    }

    fn apply_build_context(&mut self, spec: &ResolvedBuildSpec) {
        let workspace_root = resolve_workspace_root(spec);
        self.workspace_root = Some(workspace_root.clone());
        if Path::new(&self.output.path).is_relative() {
            self.output.path = Path::new(&workspace_root)
                .join(&self.output.path)
                .display()
                .to_string();
        }
        self.execution_backend = execution_backend_for_spec(
            spec,
            &self.execution_backend,
            self.execution_backend_explicit,
        );
        self.build_version = spec.identity.version.clone();
        self.build_branch = spec.metadata.branch.clone();
        self.build_target = spec.metadata.target.clone();
        self.build_profile = spec.metadata.profile.clone();
    }

    fn validate_release_invariants(&self) -> Result<(), ArtifactProviderError> {
        if let ArtifactExecutionBackend::Docker(docker) = &self.execution_backend {
            if docker.image.trim().is_empty() {
                return Err(ArtifactProviderError::new(
                    ArtifactProviderErrorKind::PolicyBlocked,
                    "artifact docker execution requires a non-empty image",
                ));
            }
            if self.workspace_root.as_deref().is_none_or(str::is_empty) {
                return Err(ArtifactProviderError::new(
                    ArtifactProviderErrorKind::RuntimeState,
                    "artifact docker execution requires a resolved workspace root",
                ));
            }
        }
        Ok(())
    }
}

fn execution_backend_for_spec(
    spec: &ResolvedBuildSpec,
    current: &ArtifactExecutionBackend,
    explicit: bool,
) -> ArtifactExecutionBackend {
    if explicit {
        return match current {
            ArtifactExecutionBackend::Docker(docker) => {
                ArtifactExecutionBackend::Docker(if docker.image.is_empty() {
                    spec.policy
                        .execution
                        .docker
                        .as_ref()
                        .map(ArtifactDockerExecution::new)
                        .unwrap_or_else(|| docker.clone())
                } else {
                    docker.clone()
                })
            }
            ArtifactExecutionBackend::Host => ArtifactExecutionBackend::Host,
        };
    }
    match current {
        ArtifactExecutionBackend::Docker(docker) => {
            ArtifactExecutionBackend::Docker(if docker.image.is_empty() {
                spec.policy
                    .execution
                    .docker
                    .as_ref()
                    .map(ArtifactDockerExecution::new)
                    .unwrap_or_else(|| docker.clone())
            } else {
                docker.clone()
            })
        }
        ArtifactExecutionBackend::Host => spec
            .policy
            .execution
            .docker
            .as_ref()
            .map(|docker| ArtifactExecutionBackend::Docker(ArtifactDockerExecution::new(docker)))
            .unwrap_or(ArtifactExecutionBackend::Host),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactDependencyContract {
    pub artifact: ArtifactRef,
}

impl ArtifactDependencyContract {
    pub fn from_ref(artifact: ArtifactRef) -> Self {
        Self { artifact }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactOutputContract {
    pub path: String,
    pub kind: ArtifactOutputKind,
}

impl ArtifactOutputContract {
    pub fn from_spec(artifact: &ArtifactSpec) -> Self {
        Self {
            path: artifact.output.path.clone(),
            kind: match &artifact.definition {
                ArtifactDefinition::Rust(rust) => match rust.variant {
                    ArtifactVariantSpec::File => ArtifactOutputKind::File,
                    ArtifactVariantSpec::Directory => ArtifactOutputKind::Directory,
                },
                _ => ArtifactOutputKind::File,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactOutputKind {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactExecutionBackend {
    Host,
    Docker(ArtifactDockerExecution),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactDockerExecution {
    pub image: String,
}

impl ArtifactDockerExecution {
    pub fn new(spec: &DockerExecutionSpec) -> Self {
        Self {
            image: spec.image.clone(),
        }
    }

    pub fn from_artifact(spec: &gaia_spec::DockerArtifactExecutionSpec) -> Self {
        Self {
            image: spec.image.clone().unwrap_or_default(),
        }
    }
}

pub(crate) fn resolve_workspace_root(spec: &ResolvedBuildSpec) -> String {
    let workspace_root = PathBuf::from(&spec.workspace.root_dir);
    fs::canonicalize(&workspace_root)
        .unwrap_or(workspace_root)
        .display()
        .to_string()
}
