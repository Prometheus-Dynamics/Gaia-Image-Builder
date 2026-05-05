use crate::{ArtifactProviderKind, ImageProviderKind, SourceProviderKind};

pub const DEFAULT_COMMAND_RETRY_ATTEMPTS: u32 = 1;
pub const DEFAULT_COMMAND_RETRY_BACKOFF_MS: u64 = 0;
pub const DEFAULT_COMMAND_RETRY_BACKOFF_STRATEGY: RetryBackoffStrategySpec =
    RetryBackoffStrategySpec::Fixed;

pub const DEFAULT_RUST_PROVIDER_TIMEOUT_SECONDS: u64 = 300;
pub const DEFAULT_GIT_PROVIDER_TIMEOUT_SECONDS: u64 = 60;
pub const DEFAULT_ARCHIVE_PROVIDER_TIMEOUT_SECONDS: u64 = 120;
pub const DEFAULT_DOWNLOAD_PROVIDER_TIMEOUT_SECONDS: u64 = 120;
pub const DEFAULT_GO_PROVIDER_TIMEOUT_SECONDS: u64 = 300;
pub const DEFAULT_JAVA_PROVIDER_TIMEOUT_SECONDS: u64 = 300;
pub const DEFAULT_NODE_PROVIDER_TIMEOUT_SECONDS: u64 = 300;
pub const DEFAULT_PYTHON_PROVIDER_TIMEOUT_SECONDS: u64 = 300;
pub const DEFAULT_BUILDROOT_PROVIDER_TIMEOUT_SECONDS: u64 = 900;
pub const DEFAULT_STARTING_POINT_PROVIDER_TIMEOUT_SECONDS: u64 = 120;
pub const DEFAULT_PROVIDER_LOCAL_JOBS: u32 = 0;

pub const DEFAULT_OUTPUT_RETENTION_STDOUT_BYTES: usize = 1024 * 1024;
pub const DEFAULT_OUTPUT_RETENTION_STDERR_BYTES: usize = 1024 * 1024;
pub const DEFAULT_OUTPUT_RETENTION_STDOUT_LINES: usize = 1_000;
pub const DEFAULT_OUTPUT_RETENTION_STDERR_LINES: usize = 1_000;
pub const DEFAULT_OUTPUT_RETENTION_FAILURE_TAIL_LINES: usize = 100;
pub const DEFAULT_OUTPUT_RETENTION_POLICY: OutputRetentionPolicySpec = OutputRetentionPolicySpec {
    stdout_bytes: DEFAULT_OUTPUT_RETENTION_STDOUT_BYTES,
    stderr_bytes: DEFAULT_OUTPUT_RETENTION_STDERR_BYTES,
    stdout_lines: DEFAULT_OUTPUT_RETENTION_STDOUT_LINES,
    stderr_lines: DEFAULT_OUTPUT_RETENTION_STDERR_LINES,
    failure_tail_lines: DEFAULT_OUTPUT_RETENTION_FAILURE_TAIL_LINES,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildPolicySpec {
    pub preset: PresetSelectionSpec,
    pub interpolation: InterpolationSpec,
    pub precedence: PrecedencePolicySpec,
    pub failure: FailureHandlingPolicySpec,
    pub execution: ExecutionPolicySpec,
    pub providers: ProviderExecutionPolicySpec,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutionPolicySpec {
    pub jobs: u32,
    pub docker: Option<DockerExecutionSpec>,
    pub output_retention: OutputRetentionPolicySpec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerExecutionSpec {
    pub image: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputRetentionPolicySpec {
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub stdout_lines: usize,
    pub stderr_lines: usize,
    pub failure_tail_lines: usize,
}

impl Default for OutputRetentionPolicySpec {
    fn default() -> Self {
        DEFAULT_OUTPUT_RETENTION_POLICY
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PresetSelectionSpec {
    pub selected: Option<String>,
    pub applied: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InterpolationSpec {
    pub allow_unresolved: bool,
    pub values: Vec<(String, String)>,
    pub unresolved: Vec<UnresolvedInterpolationSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedInterpolationSpec {
    pub location: String,
    pub token: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderExecutionPolicySpec {
    pub rust: RustProviderPolicySpec,
    pub git: GitProviderPolicySpec,
    pub archive: CommandProviderPolicySpec,
    pub download: CommandProviderPolicySpec,
    pub go: CommandProviderPolicySpec,
    pub java: CommandProviderPolicySpec,
    pub node: CommandProviderPolicySpec,
    pub python: CommandProviderPolicySpec,
    pub buildroot: CommandProviderPolicySpec,
    pub starting_point: CommandProviderPolicySpec,
}

impl ProviderExecutionPolicySpec {
    pub fn artifact_command_policy(
        &self,
        provider: ArtifactProviderKind,
    ) -> ResolvedCommandPolicySpec {
        match provider {
            ArtifactProviderKind::Rust => ResolvedCommandPolicySpec::from(&self.rust),
            ArtifactProviderKind::Go => ResolvedCommandPolicySpec::from(&self.go),
            ArtifactProviderKind::Java => ResolvedCommandPolicySpec::from(&self.java),
            ArtifactProviderKind::Node => ResolvedCommandPolicySpec::from(&self.node),
            ArtifactProviderKind::Python => ResolvedCommandPolicySpec::from(&self.python),
        }
    }

    pub fn source_command_policy(&self, provider: SourceProviderKind) -> ResolvedCommandPolicySpec {
        match provider {
            SourceProviderKind::Git => ResolvedCommandPolicySpec::from(&self.git),
            SourceProviderKind::Archive => ResolvedCommandPolicySpec::from(&self.archive),
            SourceProviderKind::Download => ResolvedCommandPolicySpec::from(&self.download),
            SourceProviderKind::Path => ResolvedCommandPolicySpec::default(),
        }
    }

    pub fn image_command_policy(&self, provider: ImageProviderKind) -> ResolvedCommandPolicySpec {
        match provider {
            ImageProviderKind::Buildroot => ResolvedCommandPolicySpec::from(&self.buildroot),
            ImageProviderKind::StartingPoint => {
                ResolvedCommandPolicySpec::from(&self.starting_point)
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedCommandPolicySpec {
    pub retry_attempts: u32,
    pub retry_backoff_ms: u64,
    pub retry_backoff_strategy: RetryBackoffStrategySpec,
    pub timeout_seconds: u64,
    pub local_jobs: u32,
    pub download_dir: Option<String>,
    pub ccache: BuildrootCcachePolicySpec,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RetryBackoffStrategySpec {
    #[default]
    Fixed,
    Exponential,
}

impl RetryBackoffStrategySpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::Exponential => "exponential",
        }
    }
}

impl std::fmt::Display for RetryBackoffStrategySpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RustProviderPolicySpec {
    pub allow_nested_build: bool,
    pub retry_attempts: u32,
    pub retry_backoff_ms: u64,
    pub retry_backoff_strategy: RetryBackoffStrategySpec,
    pub timeout_seconds: u64,
}

impl From<&RustProviderPolicySpec> for ResolvedCommandPolicySpec {
    fn from(policy: &RustProviderPolicySpec) -> Self {
        Self {
            retry_attempts: policy.retry_attempts,
            retry_backoff_ms: policy.retry_backoff_ms,
            retry_backoff_strategy: policy.retry_backoff_strategy,
            timeout_seconds: policy.timeout_seconds,
            local_jobs: 0,
            download_dir: None,
            ccache: BuildrootCcachePolicySpec::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitProviderPolicySpec {
    pub allow_remote_resolution: bool,
    pub retry_attempts: u32,
    pub retry_backoff_ms: u64,
    pub retry_backoff_strategy: RetryBackoffStrategySpec,
    pub timeout_seconds: u64,
}

impl From<&GitProviderPolicySpec> for ResolvedCommandPolicySpec {
    fn from(policy: &GitProviderPolicySpec) -> Self {
        Self {
            retry_attempts: policy.retry_attempts,
            retry_backoff_ms: policy.retry_backoff_ms,
            retry_backoff_strategy: policy.retry_backoff_strategy,
            timeout_seconds: policy.timeout_seconds,
            local_jobs: 0,
            download_dir: None,
            ccache: BuildrootCcachePolicySpec::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandProviderPolicySpec {
    pub retry_attempts: u32,
    pub retry_backoff_ms: u64,
    pub retry_backoff_strategy: RetryBackoffStrategySpec,
    pub timeout_seconds: u64,
    pub local_jobs: u32,
    pub download_dir: Option<String>,
    pub ccache: BuildrootCcachePolicySpec,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildrootCcachePolicySpec {
    pub enabled: bool,
    pub dir: Option<String>,
}

impl From<&CommandProviderPolicySpec> for ResolvedCommandPolicySpec {
    fn from(policy: &CommandProviderPolicySpec) -> Self {
        Self {
            retry_attempts: policy.retry_attempts,
            retry_backoff_ms: policy.retry_backoff_ms,
            retry_backoff_strategy: policy.retry_backoff_strategy,
            timeout_seconds: policy.timeout_seconds,
            local_jobs: policy.local_jobs,
            download_dir: policy.download_dir.clone(),
            ccache: policy.ccache.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureHandlingPolicySpec {
    pub rollback_on_error: bool,
    pub preserve_failed_outputs: bool,
    pub rollback_domains: Vec<RollbackDomain>,
}

impl Default for FailureHandlingPolicySpec {
    fn default() -> Self {
        Self {
            rollback_on_error: true,
            preserve_failed_outputs: false,
            rollback_domains: RollbackDomain::all(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RollbackDomain {
    Sources,
    Artifacts,
    Installs,
    Stage,
    Images,
    Checkpoints,
}

impl RollbackDomain {
    pub fn all() -> Vec<Self> {
        vec![
            Self::Sources,
            Self::Artifacts,
            Self::Installs,
            Self::Stage,
            Self::Images,
            Self::Checkpoints,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sources => "sources",
            Self::Artifacts => "artifacts",
            Self::Installs => "installs",
            Self::Stage => "stage",
            Self::Images => "images",
            Self::Checkpoints => "checkpoints",
        }
    }
}

impl std::fmt::Display for RollbackDomain {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrecedencePolicySpec {
    pub layers: Vec<PrecedenceLayerSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrecedenceLayerSpec {
    pub source: PrecedenceSource,
    pub applies_to: Vec<PrecedenceTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrecedenceSource {
    ConfigDefaults,
    SelectedPreset,
    EnvFiles,
    InlineEnv,
    ProcessEnv,
    CliEnvOverrides,
    CliSetOverrides,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrecedenceTarget {
    PresetSelection,
    Environment,
    Interpolation,
    Metadata,
    Provenance,
    Workspace,
    ImageOutput,
    Selection,
}

impl Default for PrecedenceLayerSpec {
    fn default() -> Self {
        Self {
            source: PrecedenceSource::ConfigDefaults,
            applies_to: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_policy_resolves_artifact_image_and_source_command_settings() {
        let mut providers = ProviderExecutionPolicySpec::default();
        providers.rust.retry_attempts = 3;
        providers.rust.retry_backoff_ms = 25;
        providers.rust.retry_backoff_strategy = RetryBackoffStrategySpec::Exponential;
        providers.rust.timeout_seconds = 120;
        providers.buildroot.retry_attempts = 2;
        providers.buildroot.timeout_seconds = 900;
        providers.buildroot.local_jobs = 2;
        providers.download.retry_attempts = 4;
        providers.download.timeout_seconds = 60;

        assert_eq!(
            providers.artifact_command_policy(ArtifactProviderKind::Rust),
            ResolvedCommandPolicySpec {
                retry_attempts: 3,
                retry_backoff_ms: 25,
                retry_backoff_strategy: RetryBackoffStrategySpec::Exponential,
                timeout_seconds: 120,
                local_jobs: 0,
                download_dir: None,
                ccache: BuildrootCcachePolicySpec::default(),
            }
        );
        let buildroot_policy = providers.image_command_policy(ImageProviderKind::Buildroot);
        assert_eq!(buildroot_policy.timeout_seconds, 900);
        assert_eq!(buildroot_policy.local_jobs, 2);
        assert_eq!(
            providers
                .source_command_policy(SourceProviderKind::Download)
                .retry_attempts,
            4
        );
        assert_eq!(
            providers.source_command_policy(SourceProviderKind::Path),
            ResolvedCommandPolicySpec::default()
        );
    }
}
