mod artifact;
mod checkpoints;
mod clean;
mod ids;
mod image;
mod inputs;
mod install;
mod metadata;
mod policy;
mod provenance;
mod reporting;
mod selection;
mod source;
mod stage;
mod state;
mod workspace;

pub use artifact::{
    ArtifactDefinition, ArtifactExecutionSpec, ArtifactInstallClassSpec,
    ArtifactInstallIdentitySpec, ArtifactOutputSpec, ArtifactProviderKind, ArtifactRef,
    ArtifactSpec, ArtifactVariantSpec, BuildModeSpec, DockerArtifactExecutionSpec, GoArtifactSpec,
    JavaArtifactSpec, NodeArtifactSpec, PythonArtifactSpec, RustArtifactSpec,
};
pub use checkpoints::{
    CheckpointAnchorRef, CheckpointBackendRef, CheckpointId, CheckpointPointSpec, CheckpointPolicy,
    CheckpointSpec,
};
pub use clean::{CleanProfileSpec, CleanSpec};
pub use ids::{ArtifactId, BuildId, IdError, InstallId, SourceId, StageItemId};
pub use image::{
    BuildrootExpectedImageFormatSpec, BuildrootExpectedImageSpec, BuildrootExternalTreeModeSpec,
    BuildrootImageSpec, ImageDefinition, ImageFeedSpec, ImageOutputSpec, ImageProviderKind,
    ImageSpec, StartingPointImageSpec, StartingPointOutputModeSpec, StartingPointPackagesSpec,
    StartingPointRootfsValidationModeSpec,
};
pub use inputs::{InputKindSpec, InputOptionSpec, InputSpec};
pub use install::{InstallEntrySpec, InstallSpec};
pub use metadata::{BuildMetadataSpec, ProductIdentitySpec};
pub use policy::{
    BuildPolicySpec, CommandProviderPolicySpec, DEFAULT_ARCHIVE_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_BUILDROOT_PROVIDER_TIMEOUT_SECONDS, DEFAULT_COMMAND_RETRY_ATTEMPTS,
    DEFAULT_COMMAND_RETRY_BACKOFF_MS, DEFAULT_COMMAND_RETRY_BACKOFF_STRATEGY,
    DEFAULT_DOWNLOAD_PROVIDER_TIMEOUT_SECONDS, DEFAULT_GIT_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_GO_PROVIDER_TIMEOUT_SECONDS, DEFAULT_JAVA_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_NODE_PROVIDER_TIMEOUT_SECONDS, DEFAULT_OUTPUT_RETENTION_FAILURE_TAIL_LINES,
    DEFAULT_OUTPUT_RETENTION_POLICY, DEFAULT_OUTPUT_RETENTION_STDERR_BYTES,
    DEFAULT_OUTPUT_RETENTION_STDERR_LINES, DEFAULT_OUTPUT_RETENTION_STDOUT_BYTES,
    DEFAULT_OUTPUT_RETENTION_STDOUT_LINES, DEFAULT_PROVIDER_LOCAL_JOBS,
    DEFAULT_PYTHON_PROVIDER_TIMEOUT_SECONDS, DEFAULT_RUST_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_STARTING_POINT_PROVIDER_TIMEOUT_SECONDS, DockerExecutionSpec, ExecutionPolicySpec,
    FailureHandlingPolicySpec, GitProviderPolicySpec, InterpolationSpec, OutputRetentionPolicySpec,
    PrecedenceLayerSpec, PrecedencePolicySpec, PrecedenceSource, PrecedenceTarget,
    PresetSelectionSpec, ProviderExecutionPolicySpec, ResolvedCommandPolicySpec,
    RetryBackoffStrategySpec, RollbackDomain, RustProviderPolicySpec, UnresolvedInterpolationSpec,
};
pub use provenance::{ProvenanceIdentitySpec, ProvenanceSpec};
pub use reporting::{PostBuildHookSpec, ReportingOutputsSpec, ReportingSpec, SecretMaskingSpec};
pub use selection::SelectionSpec;
pub use source::{
    ArchiveSourceSpec, DownloadSourceSpec, GitSourceSpec, PathSourceSpec, SourceDefinition,
    SourcePinPolicySpec, SourceProviderKind, SourceRef, SourceRefreshPolicySpec, SourceSpec,
};
pub use stage::{
    StageContentOriginSpec, StageEnvSetSpec, StageFileSpec, StageServiceSpec, StageSpec,
};
pub use state::KeyValueState;
pub use workspace::{
    CleanPolicy, WorkspaceNamedPathSpec, WorkspacePathError, WorkspacePathKindSpec, WorkspaceSpec,
    resolve_workspace_path,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBuildSpec {
    pub identity: BuildIdentity,
    pub selection: SelectionSpec,
    pub metadata: BuildMetadataSpec,
    pub inputs: InputSpec,
    pub policy: BuildPolicySpec,
    pub clean: CleanSpec,
    pub provenance: ProvenanceSpec,
    pub workspace: WorkspaceSpec,
    pub sources: Vec<SourceSpec>,
    pub artifacts: Vec<ArtifactSpec>,
    pub install: InstallSpec,
    pub stage: StageSpec,
    pub image: ImageSpec,
    pub checkpoints: CheckpointSpec,
    pub reporting: ReportingSpec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildIdentity {
    pub id: BuildId,
    pub build_name: String,
    pub display_name: String,
    pub version: Option<String>,
}

impl ResolvedBuildSpec {
    pub fn new(name: impl Into<String>) -> Self {
        let build_name = name.into();
        let id = BuildId::new(build_name.clone());
        Self {
            identity: BuildIdentity {
                id,
                build_name: build_name.clone(),
                display_name: build_name,
                version: None,
            },
            selection: SelectionSpec::default(),
            metadata: BuildMetadataSpec::default(),
            inputs: InputSpec::default(),
            policy: BuildPolicySpec::default(),
            clean: CleanSpec::default(),
            provenance: ProvenanceSpec::default(),
            workspace: WorkspaceSpec::default(),
            sources: Vec::new(),
            artifacts: Vec::new(),
            install: InstallSpec::default(),
            stage: StageSpec::default(),
            image: ImageSpec::new(ImageDefinition::Buildroot(BuildrootImageSpec::default())),
            checkpoints: CheckpointSpec::default(),
            reporting: ReportingSpec::default(),
        }
    }

    pub fn build_name(&self) -> &str {
        &self.identity.build_name
    }

    pub fn display_name(&self) -> &str {
        &self.identity.display_name
    }
}
