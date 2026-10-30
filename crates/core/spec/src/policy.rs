use crate::{ArtifactProviderKind, ImageProviderKind};

pub const DEFAULT_COMMAND_RETRY_ATTEMPTS: u32 = 1;
pub const DEFAULT_COMMAND_RETRY_BACKOFF_MS: u64 = 0;

pub const DEFAULT_RUST_PROVIDER_TIMEOUT_SECONDS: u64 = 300;
pub const DEFAULT_GIT_PROVIDER_TIMEOUT_SECONDS: u64 = 1800;
pub const DEFAULT_ARCHIVE_PROVIDER_TIMEOUT_SECONDS: u64 = 120;
pub const DEFAULT_DOWNLOAD_PROVIDER_TIMEOUT_SECONDS: u64 = 120;
pub const DEFAULT_GO_PROVIDER_TIMEOUT_SECONDS: u64 = 300;
pub const DEFAULT_JAVA_PROVIDER_TIMEOUT_SECONDS: u64 = 300;
pub const DEFAULT_NODE_PROVIDER_TIMEOUT_SECONDS: u64 = 300;
pub const DEFAULT_PYTHON_PROVIDER_TIMEOUT_SECONDS: u64 = 300;
pub const DEFAULT_BUILDROOT_PROVIDER_TIMEOUT_SECONDS: u64 = 900;
pub const DEFAULT_STARTING_POINT_PROVIDER_TIMEOUT_SECONDS: u64 = 120;

pub(crate) const DEFAULT_OUTPUT_RETENTION_STDOUT_BYTES: usize = 1024 * 1024;
pub(crate) const DEFAULT_OUTPUT_RETENTION_STDERR_BYTES: usize = 1024 * 1024;
pub(crate) const DEFAULT_OUTPUT_RETENTION_STDOUT_LINES: usize = 1_000;
pub(crate) const DEFAULT_OUTPUT_RETENTION_STDERR_LINES: usize = 1_000;
pub(crate) const DEFAULT_OUTPUT_RETENTION_FAILURE_TAIL_LINES: usize = 100;
pub(crate) const DEFAULT_OUTPUT_RETENTION_POLICY: OutputRetentionPolicySpec =
    OutputRetentionPolicySpec {
        stdout_bytes: DEFAULT_OUTPUT_RETENTION_STDOUT_BYTES,
        stderr_bytes: DEFAULT_OUTPUT_RETENTION_STDERR_BYTES,
        stdout_lines: DEFAULT_OUTPUT_RETENTION_STDOUT_LINES,
        stderr_lines: DEFAULT_OUTPUT_RETENTION_STDERR_LINES,
        failure_tail_lines: DEFAULT_OUTPUT_RETENTION_FAILURE_TAIL_LINES,
    };

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct BuildPolicySpec {
    pub preset: PresetSelectionSpec,
    pub interpolation: InterpolationSpec,
    pub precedence: PrecedencePolicySpec,
    pub failure: FailureHandlingPolicySpec,
    pub execution: ExecutionPolicySpec,
    pub providers: ProviderExecutionPolicySpec,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ExecutionPolicySpec {
    pub jobs: u32,
    /// No network for the whole run (`[execution] offline`, `--offline`).
    pub offline: bool,
    /// Buildroot RAM trees go to a tree named by the build's identity, so
    /// checkouts of one build share it and its pinned package-cache entries
    /// (`[execution] canonical_tree`). Off by default.
    pub canonical_tree: bool,
    /// `gaia prefetch` / `gaia run --prefetch`: plan only the sources and the
    /// Buildroot base, and download what they need instead of building.
    /// Set by the command, never by the build config.
    pub prefetch: bool,
    pub docker: Option<DockerExecutionSpec>,
    pub output_retention: OutputRetentionPolicySpec,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DockerExecutionSpec {
    pub image: String,
    /// One container per image, mounts and user for the run, with each command
    /// run by `docker exec` (`[providers.docker] reuse_container`).
    #[serde(skip_serializing_if = "crate::fingerprint::is_default")]
    pub reuse_container: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct PresetSelectionSpec {
    pub selected: Option<String>,
    pub applied: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct InterpolationSpec {
    pub allow_unresolved: bool,
    pub values: Vec<(String, String)>,
    pub unresolved: Vec<UnresolvedInterpolationSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UnresolvedInterpolationSpec {
    pub location: String,
    pub token: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
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

    pub fn image_command_policy(&self, provider: ImageProviderKind) -> ResolvedCommandPolicySpec {
        match provider {
            ImageProviderKind::Buildroot => ResolvedCommandPolicySpec::from(&self.buildroot),
            ImageProviderKind::StartingPoint => {
                ResolvedCommandPolicySpec::from(&self.starting_point)
            }
            // No image provider runs commands of its own.
            ImageProviderKind::None => ResolvedCommandPolicySpec::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ResolvedCommandPolicySpec {
    pub retry_attempts: u32,
    pub retry_backoff_ms: u64,
    pub retry_backoff_strategy: RetryBackoffStrategySpec,
    pub timeout_seconds: u64,
    pub local_jobs: u32,
    pub download_dir: Option<String>,
    pub ccache: BuildrootCcachePolicySpec,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
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

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct RustProviderPolicySpec {
    pub allow_nested_build: bool,
    /// Build nested cargo artifacts that share a workspace, target triple,
    /// profile and execution backend with one `cargo build -p a -p b ...`.
    /// Opt-in: cargo unifies dependency features across the batch.
    pub batch_builds: bool,
    /// Build every source's cargo artifacts into one target dir per
    /// toolchain, target triple, profile and execution backend under the user
    /// cache, so sources share compiled dependencies. Opt-in. A package whose
    /// name and version another source already built from a different
    /// directory keeps that source's own target dir (cargo would otherwise
    /// reuse the other package's outputs).
    pub shared_target_dir: bool,
    /// Compile cargo through sccache (`RUSTC_WRAPPER=sccache`) when sccache is
    /// available: on the host, or inside the docker image. Opt-in.
    pub sccache: bool,
    /// Strip the collected Rust artifact outputs with the target's strip tool
    /// (`<triple>-strip`, else `llvm-strip`, else `strip` for the host). Opt-in;
    /// cargo's own files are not touched.
    pub strip: bool,
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

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
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

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct CommandProviderPolicySpec {
    pub retry_attempts: u32,
    pub retry_backoff_ms: u64,
    pub retry_backoff_strategy: RetryBackoffStrategySpec,
    pub timeout_seconds: u64,
    pub local_jobs: u32,
    pub download_dir: Option<String>,
    pub ccache: BuildrootCcachePolicySpec,
    /// Buildroot only: share one compiled output tree between builds whose
    /// Buildroot inputs are identical.
    pub shared_output: bool,
    /// Buildroot only: where shared output trees live (defaults to
    /// `.gaia/cache/buildroot/shared` under the workspace root).
    pub shared_output_dir: Option<String>,
    /// Buildroot only: what to do when `olddefconfig` drops or changes a
    /// requested `config_overrides` entry.
    pub override_check: BuildrootOverrideCheckSpec,
    /// Buildroot only: what to do when the image holds fewer kernel modules
    /// than the kernel build produced (an ignored `modules_install` failure).
    pub kernel_modules_check: BuildrootOverrideCheckSpec,
    /// Buildroot only: build independent packages concurrently
    /// (`BR2_PER_PACKAGE_DIRECTORIES=y` and a top-level `make -j`).
    pub parallel_packages: bool,
    /// Buildroot only: reuse packages built by any build with the same
    /// inputs (requires `parallel_packages`).
    pub package_cache: BuildrootPackageCachePolicySpec,
    /// Buildroot only: the timeout of the long `make` that builds the image
    /// in seconds, 0 for none (`make_timeout_seconds`).
    pub make_timeout_seconds: u64,
    /// Buildroot only: attempts of that `make`. A failure is retried only when
    /// it looks transient (see `make_retry_attempts` in the docs).
    pub make_retry_attempts: u32,
    /// Buildroot only: `mkfs.erofs --workers` and the multithreaded host
    /// `erofs-utils` (see [`ErofsWorkersSpec`]).
    pub erofs_workers: ErofsWorkersSpec,
    /// Buildroot only: the `erofs_workers` text when it is not a known value.
    /// Validation reports it; the build uses `auto`.
    pub erofs_workers_invalid: Option<String>,
    /// Buildroot only: where the output tree is built.
    pub work_dir: BuildrootWorkDirPolicySpec,
    /// Buildroot only: where each host tool (ccache, pkgconf) comes from.
    pub host_tools: BuildrootHostToolsPolicySpec,
    /// Java only: where Gradle's user home lives for Docker builds.
    pub gradle_home: GradleHomeSpec,
    /// Java only: the configured `gradle_home` text when it is not a known
    /// value. Compilation falls back to `workspace`; validation reports it.
    pub gradle_home_invalid: Option<String>,
    /// Whether `gradle_home` is set in this provider's table. Only the Java
    /// table reads it; validation warns about it anywhere else.
    pub gradle_home_configured: bool,
}

/// How many threads `mkfs.erofs` uses for an EROFS rootfs image
/// (`[providers.buildroot] erofs_workers`). The image is byte-identical for
/// every count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub enum ErofsWorkersSpec {
    /// The smaller of 8 and the cores the build host has (the default).
    #[default]
    Auto,
    /// No multithreading: `mkfs.erofs` is single-threaded and erofs-utils is
    /// built without `--enable-multithreading`.
    Off,
    /// This many threads (at least 1).
    Count(u32),
}

impl ErofsWorkersSpec {
    /// Parses `auto`, `off`, `0` (off) or a thread count.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "auto" => Some(Self::Auto),
            "off" | "0" => Some(Self::Off),
            other => other
                .parse::<u32>()
                .ok()
                .filter(|count| *count > 0)
                .map(Self::Count),
        }
    }
}

/// Where Gradle's user home (dependency and wrapper caches) lives for
/// Docker-built Java artifacts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub enum GradleHomeSpec {
    /// `<workspace>/.gaia/docker-home/.gradle`, per workspace (default).
    #[default]
    Workspace,
    /// `<user cache root>/gradle-home`, shared by the user's workspaces and
    /// mounted into the container at the same path.
    UserCache,
}

impl GradleHomeSpec {
    /// Parses a `gradle_home` value (`workspace` or `user-cache`).
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "workspace" => Some(Self::Workspace),
            "user-cache" => Some(Self::UserCache),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::UserCache => "user-cache",
        }
    }
}

/// Where the Buildroot output tree is built.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BuildrootWorkDirPolicySpec {
    /// "disk" (the build dir, default), "ram" (tmpfs), or a directory.
    pub work_dir: String,
    /// Most RAM a "ram" tree may use, e.g. "60G".
    pub ram_budget: Option<String>,
    /// Keep a "ram" tree after a build for fast rebuilds.
    pub keep_ram_tree: bool,
}

impl Default for BuildrootWorkDirPolicySpec {
    fn default() -> Self {
        Self {
            work_dir: "disk".into(),
            ram_budget: None,
            keep_ram_tree: true,
        }
    }
}

/// Host tools a `[providers.buildroot.host_tools]` policy can name.
pub const KNOWN_HOST_TOOLS: &[&str] = &["ccache", "pkgconf"];

/// Where Buildroot's host tools come from: the build environment's own
/// ("system"), Buildroot's build ("build"), or an error ("fail"), tried in
/// order, per tool.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BuildrootHostToolsPolicySpec {
    /// Policy for tools not listed in `tools`, e.g. ["build"].
    pub default: Vec<HostToolStepSpec>,
    /// Per tool, e.g. "ccache" -> [System, Build].
    pub tools: std::collections::BTreeMap<String, Vec<HostToolStepSpec>>,
    /// Policy strings that did not parse, as (key, raw text) pairs; the key
    /// is `default` or a tool name. Their steps compile to `[Fail]`, and
    /// validation reports them.
    pub invalid: Vec<(String, String)>,
}

impl Default for BuildrootHostToolsPolicySpec {
    fn default() -> Self {
        Self {
            default: vec![HostToolStepSpec::Build],
            tools: std::collections::BTreeMap::new(),
            invalid: Vec::new(),
        }
    }
}

impl BuildrootHostToolsPolicySpec {
    pub fn steps_for(&self, tool: &str) -> &[HostToolStepSpec] {
        self.tools
            .get(tool)
            .map(Vec::as_slice)
            .unwrap_or(&self.default)
    }
}

/// One step of a host tool policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub enum HostToolStepSpec {
    System,
    Build,
    Fail,
}

impl HostToolStepSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Build => "build",
            Self::Fail => "fail",
        }
    }

    /// Parses a comma-separated, non-empty list of steps such as
    /// `system,build`.
    pub fn parse_list(text: &str) -> Result<Vec<Self>, String> {
        if text.trim().is_empty() {
            return Err("the policy is empty; list at least one of system, build, fail".into());
        }
        text.split(',')
            .map(|part| match part.trim() {
                "system" => Ok(Self::System),
                "build" => Ok(Self::Build),
                "fail" => Ok(Self::Fail),
                other => Err(format!(
                    "unknown step `{other}` (expected system, build or fail)"
                )),
            })
            .collect()
    }
}

/// `[providers.buildroot.package_cache]`: a cache of built Buildroot
/// packages shared by every build of the user.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct BuildrootPackageCachePolicySpec {
    pub enabled: bool,
    /// Where packages are stored unless listed in `project_packages` or
    /// `system_packages`.
    pub level: PackageCacheLevelSpec,
    /// The system level, shared by every project of the user; defaults to
    /// `<user cache root>/buildroot/packages`.
    pub system_dir: Option<String>,
    /// The project level; defaults to `<workspace>/.gaia/cache/buildroot/packages`.
    pub project_dir: Option<String>,
    /// Packages (globs) stored at the project level only.
    pub project_packages: Vec<String>,
    /// Packages (globs) stored at the system level even when `level` is
    /// `project`.
    pub system_packages: Vec<String>,
    /// Size to keep at each level (for example `100G`); least recently
    /// used packages are evicted beyond it. Unset, each level is kept at
    /// half its filesystem's free space plus what it holds, between 2 GiB
    /// and 40 GiB.
    pub max_size: Option<String>,
}

impl BuildrootPackageCachePolicySpec {
    /// The level a package is stored at.
    pub fn level_of(&self, package: &str) -> PackageCacheLevelSpec {
        let listed = |patterns: &[String]| {
            patterns
                .iter()
                .any(|pattern| crate::wildcard_match(pattern, package))
        };
        if listed(&self.project_packages) {
            PackageCacheLevelSpec::Project
        } else if listed(&self.system_packages) {
            PackageCacheLevelSpec::System
        } else {
            self.level
        }
    }
}

/// The two package cache levels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub enum PackageCacheLevelSpec {
    /// Shared by every project of the user.
    #[default]
    System,
    /// This project's own.
    Project,
}

impl PackageCacheLevelSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Project => "project",
        }
    }
}

/// `[providers.buildroot] override_check`: how Gaia reacts when the final
/// Buildroot `.config` does not contain a requested `config_overrides` entry
/// (usually an unmet `depends on`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize)]
pub enum BuildrootOverrideCheckSpec {
    /// Fail the image operation before the long `make`.
    #[default]
    Error,
    /// Report the dropped entries in the run output and report, then build.
    Warn,
    /// Do not compare.
    Off,
}

impl BuildrootOverrideCheckSpec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Off => "off",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct BuildrootCcachePolicySpec {
    pub enabled: bool,
    /// Defaults to a cache shared by every workspace of the user.
    pub dir: Option<String>,
    /// ccache `max_size` (for example `50G`), written to the cache's
    /// `ccache.conf`. Unset: computed from the free space at run start.
    pub max_size: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FailureHandlingPolicySpec {
    pub rollback_on_error: bool,
    pub preserve_failed_outputs: bool,
    pub rollback_domains: Vec<RollbackDomain>,
    /// After a failure or cancellation, also clean the outputs of operations
    /// that completed earlier in the run (within `rollback_domains`). Off by
    /// default: finished work is kept and recorded for reuse, and only the
    /// failed operation's own partial outputs are cleaned.
    pub rollback_completed: bool,
    /// After a failure, let independent operations finish; only dependents
    /// of the failed operation are skipped. The run still fails.
    pub keep_going: bool,
}

impl Default for FailureHandlingPolicySpec {
    fn default() -> Self {
        Self {
            rollback_on_error: true,
            preserve_failed_outputs: false,
            rollback_completed: false,
            keep_going: false,
            rollback_domains: RollbackDomain::all(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
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

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct PrecedencePolicySpec {
    pub layers: Vec<PrecedenceLayerSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PrecedenceLayerSpec {
    pub source: PrecedenceSource,
    pub applies_to: Vec<PrecedenceTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum PrecedenceSource {
    ConfigDefaults,
    SelectedPreset,
    EnvFiles,
    InlineEnv,
    ProcessEnv,
    CliEnvOverrides,
    CliSetOverrides,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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
    }
}

#[cfg(test)]
mod host_tool_tests {
    use super::*;

    #[test]
    fn parses_ordered_step_lists() {
        assert_eq!(
            HostToolStepSpec::parse_list("system,build").expect("list"),
            vec![HostToolStepSpec::System, HostToolStepSpec::Build]
        );
        assert_eq!(
            HostToolStepSpec::parse_list(" fail ").expect("single"),
            vec![HostToolStepSpec::Fail]
        );
        assert_eq!(
            HostToolStepSpec::parse_list("build, system").expect("spaces"),
            vec![HostToolStepSpec::Build, HostToolStepSpec::System]
        );
    }

    #[test]
    fn rejects_empty_lists_and_unknown_steps() {
        assert!(HostToolStepSpec::parse_list("").is_err());
        assert!(HostToolStepSpec::parse_list("  ").is_err());
        assert!(HostToolStepSpec::parse_list("system,").is_err());
        assert!(HostToolStepSpec::parse_list("system,host").is_err());
        assert!(HostToolStepSpec::parse_list("System").is_err());
    }

    #[test]
    fn steps_for_falls_back_to_the_default_policy() {
        let mut policy = BuildrootHostToolsPolicySpec::default();
        assert_eq!(policy.steps_for("ccache"), &[HostToolStepSpec::Build]);
        policy.tools.insert(
            "ccache".into(),
            vec![HostToolStepSpec::System, HostToolStepSpec::Build],
        );
        assert_eq!(
            policy.steps_for("ccache"),
            &[HostToolStepSpec::System, HostToolStepSpec::Build]
        );
        assert_eq!(policy.steps_for("pkgconf"), &[HostToolStepSpec::Build]);
    }
}
