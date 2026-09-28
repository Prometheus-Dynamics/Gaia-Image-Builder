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
                        ArtifactDockerExecution::from_artifact(artifact.id.as_str(), docker),
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
        if let ArtifactExecutionBackend::Docker(docker) = &mut self.execution_backend {
            docker.resolve_build(spec);
        }
        self.build_version = spec.identity.version.clone();
        self.build_branch = spec.metadata.branch.clone();
        self.build_target = spec.metadata.target.clone();
        self.build_profile = spec.metadata.profile.clone();
    }

    fn validate_release_invariants(&self) -> Result<(), ArtifactProviderError> {
        if let ArtifactExecutionBackend::Docker(docker) = &self.execution_backend {
            if let Some(error) = docker
                .build
                .as_ref()
                .and_then(|build| build.hash_error.as_ref())
            {
                return Err(ArtifactProviderError::new(
                    ArtifactProviderErrorKind::PolicyBlocked,
                    error.clone(),
                ));
            }
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
            ArtifactExecutionBackend::Docker(docker) => ArtifactExecutionBackend::Docker(
                if docker.image.is_empty() && docker.build.is_none() {
                    spec.policy
                        .execution
                        .docker
                        .as_ref()
                        .map(ArtifactDockerExecution::new)
                        .unwrap_or_else(|| docker.clone())
                } else {
                    docker.clone()
                },
            ),
            ArtifactExecutionBackend::Host => ArtifactExecutionBackend::Host,
        };
    }
    match current {
        ArtifactExecutionBackend::Docker(docker) => {
            ArtifactExecutionBackend::Docker(if docker.image.is_empty() && docker.build.is_none() {
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
    /// Set when the image is built by Gaia from a Dockerfile.
    pub build: Option<ArtifactDockerImageBuild>,
}

/// A Dockerfile-backed execution image. Paths are resolved against the
/// workspace when the build context is applied; `image` on the owning
/// execution then holds the content-addressed tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactDockerImageBuild {
    pub dockerfile: String,
    pub context: Option<String>,
    /// Repository name for the tag (from `image` or the artifact id).
    pub name: String,
    /// SHA-256 of the Dockerfile and context, once resolved.
    pub content_hash: Option<String>,
    /// Why the content hash could not be computed.
    pub hash_error: Option<String>,
    /// Docker image id, once the image is present locally.
    pub image_id: Option<String>,
}

impl ArtifactDockerExecution {
    pub fn new(spec: &DockerExecutionSpec) -> Self {
        Self {
            image: spec.image.clone(),
            build: None,
        }
    }

    pub fn from_artifact(artifact_id: &str, spec: &gaia_spec::DockerArtifactExecutionSpec) -> Self {
        let build = spec
            .dockerfile
            .as_ref()
            .map(|dockerfile| ArtifactDockerImageBuild {
                dockerfile: dockerfile.clone(),
                context: spec.context.clone(),
                name: spec
                    .image
                    .as_deref()
                    .map(image_repository_name)
                    .unwrap_or(artifact_id)
                    .to_string(),
                content_hash: None,
                hash_error: None,
                image_id: None,
            });
        Self {
            // A Dockerfile-built image is tagged once its content is hashed.
            image: if build.is_some() {
                String::new()
            } else {
                spec.image.clone().unwrap_or_default()
            },
            build,
        }
    }

    fn resolve_build(&mut self, spec: &ResolvedBuildSpec) {
        let Some(build) = &mut self.build else {
            return;
        };
        let resolve = |value: &str| {
            gaia_spec::resolve_workspace_path(&spec.workspace, value)
                .map_err(|error| format!("invalid docker build path '{value}': {error}"))
        };
        let resolved = resolve(&build.dockerfile).and_then(|dockerfile| {
            let context = match &build.context {
                Some(context) => resolve(context)?,
                None => dockerfile
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default(),
            };
            Ok((dockerfile, context))
        });
        match resolved {
            Ok((dockerfile, context)) => {
                build.dockerfile = dockerfile.display().to_string();
                build.context = Some(context.display().to_string());
                match gaia_process::docker_build_context_hash(&dockerfile, &context) {
                    Ok(hash) => {
                        self.image = gaia_process::docker_local_image_tag(&build.name, &hash);
                        build.content_hash = Some(hash);
                    }
                    Err(error) => {
                        build.hash_error = Some(format!(
                            "failed to hash docker build '{}' (context '{}'): {error}",
                            dockerfile.display(),
                            context.display()
                        ));
                    }
                }
            }
            Err(error) => build.hash_error = Some(error),
        }
    }
}

/// `registry.example/helios-cross-rust194:latest` -> `helios-cross-rust194`.
fn image_repository_name(image: &str) -> &str {
    let without_digest = image.split('@').next().unwrap_or(image);
    let last = without_digest.rsplit('/').next().unwrap_or(without_digest);
    last.split(':').next().unwrap_or(last)
}

pub(crate) fn resolve_workspace_root(spec: &ResolvedBuildSpec) -> String {
    let workspace_root = PathBuf::from(&spec.workspace.root_dir);
    fs::canonicalize(&workspace_root)
        .unwrap_or(workspace_root)
        .display()
        .to_string()
}
