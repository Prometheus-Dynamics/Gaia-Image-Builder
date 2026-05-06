use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Deserializer, de};

use crate::raw_assembly::RawImageAssemblyConfig;

#[path = "raw_inputs.rs"]
mod raw_inputs;

pub use raw_inputs::*;

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawBuildConfig {
    pub build_name: String,
    pub display_name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub branch: Option<String>,
    pub target: Option<String>,
    pub profile: Option<String>,
    // Labels intentionally stay pair-shaped in raw config because they are open-ended user metadata,
    // not a closed enum domain.
    pub labels: Vec<(String, String)>,
    pub product: RawProductConfig,
    pub inputs: BTreeMap<String, RawInputOptionConfig>,
    pub preset: Option<String>,
    pub presets: BTreeMap<String, RawPresetConfig>,
    pub extends: Option<String>,
    pub imports: Vec<RawImportConfig>,
    pub env_files: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub workspace: RawWorkspaceConfig,
    pub sources: Vec<RawSourceConfig>,
    pub artifacts: Vec<RawArtifactConfig>,
    pub install: Vec<RawInstallConfig>,
    pub stage: RawStageConfig,
    pub image: RawImageConfig,
    pub checkpoints: Vec<RawCheckpointConfig>,
    pub interpolation: RawInterpolationConfig,
    pub clean: RawCleanConfig,
    pub execution: RawExecutionPolicyConfig,
    pub failure: RawFailurePolicyConfig,
    pub providers: RawProviderPoliciesConfig,
    pub provenance: RawProvenanceConfig,
    pub reporting: RawReportingConfig,
    #[serde(skip)]
    pub source_path: Option<PathBuf>,
    #[serde(skip)]
    pub requested_build: Option<String>,
    #[serde(skip)]
    // CLI env overrides stay pair-shaped because they are discovered dynamically at invocation time.
    pub env_overrides: Vec<(String, String)>,
    #[serde(skip)]
    // Explicit overrides stay loose because this is the escape hatch layer before canonical typing.
    pub explicit_overrides: Vec<(String, String)>,
    #[serde(skip)]
    // Selected input values remain name/value pairs because declared input kinds validate them later.
    pub selected_inputs: Vec<(String, String)>,
    #[serde(skip)]
    pub extends_config: Option<Box<RawBuildConfig>>,
    #[serde(skip)]
    pub imported_configs: Vec<RawImportedConfig>,
    #[serde(skip)]
    pub unresolved_tokens: Vec<RawUnresolvedInterpolation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawImportConfig {
    pub path: String,
    pub when: Option<RawWhenConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawImportedConfig {
    pub import: RawImportConfig,
    pub config: RawBuildConfig,
}

impl<'de> Deserialize<'de> for RawImportConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum ImportValue {
            Path(String),
            Table {
                path: String,
                #[serde(default)]
                when: Option<RawWhenConfig>,
            },
        }

        match ImportValue::deserialize(deserializer)? {
            ImportValue::Path(path) => Ok(Self { path, when: None }),
            ImportValue::Table { path, when } => Ok(Self { path, when }),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawPresetConfig {
    pub env_files: Vec<String>,
    pub env: BTreeMap<String, String>,
    // Preset overrides intentionally remain loose so presets can target any top-level override key.
    pub overrides: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawUnresolvedInterpolation {
    pub location: String,
    pub token: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawWorkspaceConfig {
    pub root_dir: String,
    pub build_dir: String,
    pub out_dir: String,
    pub named_paths: Vec<RawWorkspaceNamedPathConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawWorkspaceNamedPathConfig {
    pub alias: String,
    pub path: String,
    pub kind: RawWorkspacePathKind,
}

impl<'de> Deserialize<'de> for RawWorkspaceNamedPathConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize, Default)]
        #[serde(default, deny_unknown_fields)]
        struct RawWorkspaceNamedPathConfigFields {
            alias: String,
            path: String,
            kind: RawWorkspacePathKind,
        }

        let fields = RawWorkspaceNamedPathConfigFields::deserialize(deserializer).map_err(|error| {
            de::Error::custom(format!(
                "workspace.named_paths entries must use table/object form with alias/path/kind fields: {error}"
            ))
        })?;

        Ok(Self {
            alias: fields.alias,
            path: fields.path,
            kind: fields.kind,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawWorkspacePathKind {
    #[default]
    Host,
    Logical,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawProductConfig {
    pub family: Option<String>,
    pub name: Option<String>,
    pub sku: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawInterpolationConfig {
    pub allow_unresolved: bool,
    // Interpolation values intentionally remain free-form pairs because they define user-owned
    // variable namespaces rather than closed spec enums.
    pub values: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawCleanConfig {
    pub default: Option<String>,
    pub profiles: BTreeMap<String, RawCleanProfileConfig>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawCleanProfileConfig {
    pub description: Option<String>,
    pub build: bool,
    pub out: bool,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawExecutionPolicyConfig {
    pub jobs: u32,
    pub docker: RawDockerExecutionConfig,
    pub output_retention: RawOutputRetentionPolicyConfig,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawDockerExecutionConfig {
    pub enabled: bool,
    pub image: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawOutputRetentionPolicyConfig {
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub stdout_lines: usize,
    pub stderr_lines: usize,
    pub failure_tail_lines: usize,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawFailurePolicyConfig {
    pub rollback_on_error: Option<bool>,
    pub preserve_failed_outputs: Option<bool>,
    pub rollback_domains: Option<Vec<RawRollbackDomain>>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawRollbackDomain {
    Sources,
    Artifacts,
    Installs,
    Stage,
    Images,
    Checkpoints,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawProviderPoliciesConfig {
    pub rust: RawRustProviderPolicyConfig,
    pub git: RawGitProviderPolicyConfig,
    pub archive: RawCommandProviderPolicyConfig,
    pub download: RawCommandProviderPolicyConfig,
    pub go: RawCommandProviderPolicyConfig,
    pub java: RawCommandProviderPolicyConfig,
    pub node: RawCommandProviderPolicyConfig,
    pub python: RawCommandProviderPolicyConfig,
    pub buildroot: RawCommandProviderPolicyConfig,
    pub starting_point: RawCommandProviderPolicyConfig,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawRustProviderPolicyConfig {
    pub allow_nested_build: bool,
    pub retry_attempts: u32,
    pub retry_backoff_ms: u64,
    pub retry_backoff_strategy: RawRetryBackoffStrategy,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawGitProviderPolicyConfig {
    pub allow_remote_resolution: bool,
    pub retry_attempts: u32,
    pub retry_backoff_ms: u64,
    pub retry_backoff_strategy: RawRetryBackoffStrategy,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawCommandProviderPolicyConfig {
    pub retry_attempts: u32,
    pub retry_backoff_ms: u64,
    pub retry_backoff_strategy: RawRetryBackoffStrategy,
    pub timeout_seconds: u64,
    pub local_jobs: u32,
    pub download_dir: Option<String>,
    pub ccache: RawBuildrootCcachePolicyConfig,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawBuildrootCcachePolicyConfig {
    pub enabled: bool,
    pub dir: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawRetryBackoffStrategy {
    #[default]
    Fixed,
    Exponential,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawProvenanceConfig {
    pub identity: RawProvenanceIdentityConfig,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawProvenanceIdentityConfig {
    pub project: Option<String>,
    pub vendor: Option<String>,
    pub channel: Option<String>,
    // Identity labels remain free-form because provenance tags are user-defined metadata.
    pub labels: Vec<(String, String)>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RawSourceConfig {
    pub id: String,
    #[serde(flatten)]
    pub definition: RawSourceDefinition,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RawSourceDefinition {
    Git {
        repo: String,
        #[serde(default)]
        branch: Option<String>,
        #[serde(default)]
        tag: Option<String>,
        #[serde(default)]
        rev: Option<String>,
        #[serde(default)]
        subdir: Option<String>,
        #[serde(default)]
        update: bool,
        #[serde(default)]
        refresh: Option<RawSourceRefreshPolicy>,
        #[serde(default)]
        pin: Option<RawSourcePinPolicy>,
    },
    Path {
        path: String,
        #[serde(default)]
        identity_ignore: Vec<String>,
        #[serde(default)]
        refresh: Option<RawSourceRefreshPolicy>,
        #[serde(default)]
        pin: Option<RawSourcePinPolicy>,
    },
    Archive {
        path: String,
        strip_components: u32,
        #[serde(default)]
        refresh: Option<RawSourceRefreshPolicy>,
        #[serde(default)]
        pin: Option<RawSourcePinPolicy>,
    },
    Download {
        url: String,
        #[serde(default)]
        sha256: Option<String>,
        output_path: String,
        #[serde(default)]
        refresh: Option<RawSourceRefreshPolicy>,
        #[serde(default)]
        pin: Option<RawSourcePinPolicy>,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawSourceRefreshPolicy {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawSourcePinPolicy {
    Floating,
    Locked,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RawArtifactConfig {
    pub id: String,
    #[serde(default)]
    pub when: Option<RawWhenConfig>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub execution: RawArtifactExecutionConfig,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub install_name: Option<String>,
    #[serde(default)]
    pub install_class: Option<RawArtifactInstallClass>,
    #[serde(default)]
    pub install_dest_hint: Option<String>,
    pub output_path: String,
    #[serde(flatten)]
    pub definition: RawArtifactDefinition,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawArtifactExecutionConfig {
    pub backend: Option<RawArtifactExecutionBackend>,
    pub docker: RawArtifactDockerExecutionConfig,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawArtifactDockerExecutionConfig {
    pub image: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawArtifactExecutionBackend {
    Host,
    Docker,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawArtifactInstallClass {
    Binary,
    Library,
    Archive,
    Config,
    Service,
    Data,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RawArtifactDefinition {
    Rust {
        package: String,
        #[serde(default)]
        target_name: Option<String>,
        #[serde(default)]
        emit_directory: bool,
    },
    Java {
        build_target: String,
    },
    Node {
        package_dir: String,
    },
    Python {
        package_dir: String,
    },
    Go {
        package: String,
    },
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RawInstallConfig {
    pub id: String,
    #[serde(default)]
    pub when: Option<RawWhenConfig>,
    pub artifact: String,
    pub dest: String,
    #[serde(default)]
    pub replace: bool,
    #[serde(default)]
    pub mode: Option<u32>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawStageConfig {
    pub files: Vec<RawStageFileConfig>,
    pub env_sets: Vec<RawStageEnvSetConfig>,
    pub services: Vec<RawStageServiceConfig>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RawStageFileConfig {
    pub id: String,
    #[serde(default)]
    pub when: Option<RawWhenConfig>,
    pub src: String,
    pub dest: String,
    #[serde(default)]
    pub mode: Option<u32>,
    #[serde(default)]
    pub origin: Option<RawStageContentOrigin>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawStageContentOrigin {
    StaticAsset,
    Generated,
    ProviderEmitted,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RawStageEnvSetConfig {
    pub id: String,
    #[serde(default)]
    pub when: Option<RawWhenConfig>,
    pub name: String,
    // Env-set entries stay pair-shaped because env keys are inherently open-ended.
    pub entries: Vec<(String, String)>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RawStageServiceConfig {
    pub id: String,
    #[serde(default)]
    pub when: Option<RawWhenConfig>,
    pub name: String,
    pub unit_path: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct RawWhenConfig {
    pub target: Option<String>,
    pub profile: Option<String>,
    pub branch: Option<String>,
    pub image_kind: Option<RawWhenImageKind>,
    pub all: Vec<RawWhenConfig>,
    pub any: Vec<RawWhenConfig>,
    pub not: Option<Box<RawWhenConfig>>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawWhenImageKind {
    Buildroot,
    StartingPoint,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawImageConfig {
    #[serde(flatten)]
    pub definition: RawImageDefinition,
    pub feed: RawImageFeedConfig,
    pub output: RawImageOutputConfig,
    pub assembly: Option<RawImageAssemblyConfig>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RawImageDefinition {
    Buildroot {
        #[serde(default)]
        source: Option<String>,
        #[serde(default)]
        defconfig: Option<String>,
        #[serde(default)]
        defconfig_path: Option<String>,
        #[serde(default)]
        allow_fallback: bool,
        #[serde(default)]
        config_fragments: Vec<String>,
        #[serde(default)]
        config_overrides: Vec<(String, String)>,
        #[serde(default)]
        external_tree: Option<String>,
        #[serde(default)]
        external_tree_mode: Option<RawBuildrootExternalTreeMode>,
        #[serde(default)]
        expected_images: Vec<RawBuildrootExpectedImageConfig>,
    },
    StartingPoint {
        #[serde(default)]
        source: Option<String>,
        #[serde(default)]
        source_path: Option<String>,
        #[serde(default)]
        rootfs_path: String,
        #[serde(default)]
        image_partition: Option<String>,
        #[serde(default = "default_true")]
        image_read_only: bool,
        #[serde(default)]
        packages: RawStartingPointPackagesConfig,
        #[serde(default)]
        rootfs_validation_mode: Option<RawStartingPointRootfsValidationMode>,
        #[serde(default)]
        output_mode: Option<RawStartingPointOutputMode>,
    },
}

impl Default for RawImageDefinition {
    fn default() -> Self {
        Self::Buildroot {
            source: None,
            defconfig: None,
            defconfig_path: None,
            allow_fallback: false,
            config_fragments: Vec::new(),
            config_overrides: Vec::new(),
            external_tree: None,
            external_tree_mode: None,
            expected_images: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawImageFeedConfig {
    // Feed references remain raw strings here because they compile into typed ids in the canonical spec.
    pub install_entries: Vec<String>,
    pub stage_files: Vec<String>,
    pub stage_env_sets: Vec<String>,
    pub stage_services: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RawBuildrootExpectedImageConfig {
    pub name: String,
    pub format: RawBuildrootExpectedImageFormat,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawStartingPointPackagesConfig {
    pub enabled: bool,
    pub execute: bool,
    pub manager: Option<String>,
    pub release_version: Option<String>,
    pub allow_major_upgrade: bool,
    pub update: bool,
    pub dist_upgrade: bool,
    pub install: Vec<String>,
    pub remove: Vec<String>,
    pub extra_args: Vec<String>,
    pub os_release_path: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawBuildrootExpectedImageFormat {
    Tar,
    Cpio,
    Ext2,
    Ext3,
    Ext4,
    Ubifs,
    Ubi,
    Jffs2,
    Romfs,
    Cramfs,
    Cloop,
    F2fs,
    Btrfs,
    Squashfs,
    Raw,
    Kernel,
    Erofs,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawBuildrootExternalTreeMode {
    Auto,
    Required,
    Disabled,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawStartingPointRootfsValidationMode {
    RequireExists,
    RequireDirectory,
    RequireFile,
    AllowMissing,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawStartingPointOutputMode {
    CopyRootfs,
    ArchiveOnly,
    CopyAndArchive,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawImageOutputConfig {
    pub collect_dir: Option<String>,
    pub archive_name: Option<String>,
    pub emit_report: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RawCheckpointConfig {
    pub id: String,
    pub backend: Option<String>,
    pub use_policy: RawCheckpointPolicy,
    pub upload_policy: RawCheckpointPolicy,
    #[serde(default)]
    pub anchor: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RawCheckpointPolicy {
    #[default]
    Off,
    Auto,
    Always,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawReportingConfig {
    pub summary: bool,
    pub provenance: bool,
    pub manifest: bool,
    pub masking: RawReportingMaskingConfig,
    pub output_hygiene: RawOutputHygieneConfig,
    pub post_build: Option<RawPostBuildHookConfig>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawOutputHygieneConfig {
    pub large_file_threshold_bytes: Option<u64>,
    pub transient_dir_names: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawPostBuildHookConfig {
    pub script: String,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RawReportingMaskingConfig {
    pub enabled: bool,
    pub replacement: String,
    pub patterns: Vec<String>,
}

impl Default for RawReportingMaskingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            replacement: "***".into(),
            patterns: vec![
                "TOKEN".into(),
                "SECRET".into(),
                "PASSWORD".into(),
                "PRIVATE_KEY".into(),
                "API_KEY".into(),
                "ACCESS_KEY".into(),
                "CREDENTIAL".into(),
                "AUTH".into(),
            ],
        }
    }
}
