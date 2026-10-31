mod artifact;
mod assembly;
mod assembly_archive_spec;
mod assembly_compressed_fs_spec;
mod assembly_gpt_spec;
mod assembly_json;
mod assembly_oci_spec;
mod assembly_order;
mod assembly_output_spec;
mod assembly_package_spec;
mod assembly_spec;
mod assembly_ubi_spec;
mod byte_size;
mod checkpoints;
mod clean;
mod export;
mod file_mode;
pub mod fingerprint;
mod ids;
mod image;
mod inputs;
mod install;
mod metadata;
mod policy;
mod provenance;
mod raw_disk_archive;
mod redact;
mod reporting;
mod secrets;
mod selection;
mod source;
mod source_dirs;
mod stage;
mod state;
mod suggest;
mod target_arch;
mod toolchain;
mod user_cache;
mod variant;
mod workspace;

pub use artifact::{
    ArtifactDefinition, ArtifactExecutionSpec, ArtifactInstallClassSpec,
    ArtifactInstallIdentitySpec, ArtifactOutputSpec, ArtifactProviderKind, ArtifactRef,
    ArtifactSpec, ArtifactVariantSpec, BuildModeSpec, CargoGitSourceSpec,
    DockerArtifactExecutionSpec, GoArtifactSpec, JavaArtifactSpec, NodeArtifactSpec,
    PythonArtifactSpec, RustArtifactSpec, cargo_git_url_key,
};
pub use assembly::{
    AssemblyRoots, AssemblyWorkDirKeyword, assembly_work_dir_keyword, boot_tree_snapshot_dir,
    buildroot_output_dir, exclude_names, expand_simple_glob, image_checks_report_path,
    wildcard_match,
};
pub use assembly_archive_spec::{
    AssemblyArchiveMemberSourceSpec, AssemblyArchiveMemberSpec, AssemblyArchiveSpec,
    AssemblyTreeDigestToken, AssemblyValueToken, AssemblyVerityField, assembly_json_string_tokens,
    assembly_value_tokens, whole_tree_digest_token,
};
pub use assembly_compressed_fs_spec::{
    AssemblyCompressedFsSpec, EROFS_BLOCK_SIZES, ErofsCompression, SQUASHFS_BLOCK_SIZES,
    SquashfsCompression,
};
pub use assembly_gpt_spec::{
    AssemblyGptDiskSpec, AssemblyGptPartitionSpec, GptGuid, GptPartitionTypeError,
};
pub use assembly_json::AssemblyJsonValue;
pub use assembly_oci_spec::{
    AssemblyOciCompressionSpec, AssemblyOciConfigSpec, AssemblyOciFormatSpec,
    AssemblyOciLayerSource, AssemblyOciLayerSpec, AssemblyOciSpec,
};
pub use assembly_order::{
    AssemblyStep, AssemblyStepPaths, assembly_paths_overlap, assembly_step_paths,
    order_assembly_steps,
};
pub use assembly_output_spec::{
    AssemblyFilesystemKindSpec, AssemblyFilesystemSpec, AssemblyTransformKindSpec,
    AssemblyTransformSpec, VFAT_FAT_SIZES,
};
pub use assembly_package_spec::{
    AssemblyPackageControlSpec, AssemblyPackageDeviceSpec, AssemblyPackageScriptsSpec,
    AssemblyPackageSpec, DEFAULT_DEVICE_MIN_PAGE_SIZE, PACKAGE_FORMAT_DEB, PACKAGE_FORMAT_IPK,
    parse_glibc_version,
};
pub(crate) use assembly_spec::AssemblyPathTemplateContext;
pub use assembly_spec::{
    AssemblyBinarySpec, AssemblyBusyboxInitramfsSpec, AssemblyCheckSpec, AssemblyDeviceKindSpec,
    AssemblyDeviceSpec, AssemblyDirSpec, AssemblyDiskPartitionSpec, AssemblyDiskSpec,
    AssemblyDiskTruncateSpec, AssemblyEbrPlacementSpec, AssemblyFileSpec,
    AssemblyKernelModulesSpec, AssemblyPartitionTableSpec, AssemblyPathTemplate,
    AssemblySymlinkSpec, AssemblyTreeSpec, ImageAssemblySpec, MbrPartitionTypeParseError,
};
pub use assembly_ubi_spec::{
    AssemblyUbiSpec, AssemblyUbiVolumeKindSpec, AssemblyUbiVolumeSpec,
    UBI_DEFAULT_BAD_PEBS_PER_1024, UBI_HEADER_SIZE, UBI_MAX_VOLUMES, UBI_VOLUME_NAME_MAX,
    UBI_VTBL_RECORD_SIZE, UbiGeometry,
};
pub use byte_size::ByteSize;
pub use byte_size::ByteSizeParseError;
pub use checkpoints::{
    CheckpointAnchorRef, CheckpointBackendRef, CheckpointId, CheckpointPointSpec, CheckpointPolicy,
    CheckpointSpec, OciCheckpointSpec,
};
pub use clean::{CleanProfileSpec, CleanSpec};
pub use export::ExportSpec;
pub use file_mode::{FileMode, FileModeParseError};
pub use fingerprint::FingerprintDigest;
pub use fingerprint::{FINGERPRINT_SCHEME, Fingerprint, canonical_json, sha256_hex};
pub use ids::IdError;
pub use ids::{
    ArtifactId, AssemblyFilesystemId, AssemblyTreeId, BuildId, InstallId, SourceId, StageItemId,
};
pub use image::{
    BuildrootExpectedImageFormatSpec, BuildrootExpectedImageSpec, BuildrootExternalTreeModeSpec,
    BuildrootImageSpec, DEFAULT_LAYER_EROFS_OPTIONS, ImageChecksSpec, ImageDefinition,
    ImageFeedSpec, ImageHardwareSpec, ImageLayerSpec, ImageLayersSpec, ImageOutputSpec,
    ImageProviderKind, ImageSigningKind, ImageSigningSpec, ImageSpec, StartingPointImageSpec,
    StartingPointOutputModeSpec, StartingPointPackagesSpec, StartingPointRootfsValidationModeSpec,
};
pub use inputs::{InputKindSpec, InputOptionSpec, InputSpec};
pub use install::InstallEntrySpec;
pub use install::InstallSpec;
pub use metadata::{BuildMetadataSpec, ExpectedItemsSpec, ProductIdentitySpec};
pub use policy::{
    BuildPolicySpec, BuildrootCcachePolicySpec, BuildrootHostToolsPolicySpec,
    BuildrootOverrideCheckSpec, BuildrootPackageCachePolicySpec, BuildrootWorkDirPolicySpec,
    CommandProviderPolicySpec, DEFAULT_ARCHIVE_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_BUILDROOT_PROVIDER_TIMEOUT_SECONDS, DEFAULT_COMMAND_RETRY_ATTEMPTS,
    DEFAULT_COMMAND_RETRY_BACKOFF_MS, DEFAULT_DOWNLOAD_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_GIT_PROVIDER_TIMEOUT_SECONDS, DEFAULT_GO_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_JAVA_PROVIDER_TIMEOUT_SECONDS, DEFAULT_NODE_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_PYTHON_PROVIDER_TIMEOUT_SECONDS, DEFAULT_RUST_PROVIDER_TIMEOUT_SECONDS,
    DEFAULT_STARTING_POINT_PROVIDER_TIMEOUT_SECONDS, DockerExecutionSpec, ErofsWorkersSpec,
    ExecutionPolicySpec, FailureHandlingPolicySpec, GitProviderPolicySpec, GradleHomeSpec,
    HostToolStepSpec, InterpolationSpec, KNOWN_HOST_TOOLS, OutputRetentionPolicySpec,
    PackageCacheLevelSpec, PrecedenceLayerSpec, PrecedencePolicySpec, PrecedenceSource,
    PrecedenceTarget, PresetSelectionSpec, ProviderExecutionPolicySpec, ResolvedCommandPolicySpec,
    RetryBackoffStrategySpec, RollbackDomain, RustProviderPolicySpec, UnresolvedInterpolationSpec,
};
pub use provenance::{ProvenanceIdentitySpec, ProvenanceSpec};
pub use raw_disk_archive::{RawDiskArchive, raw_disk_archive};
pub use redact::redact_url_userinfo;
pub use reporting::{
    DEFAULT_LARGE_UNEXPECTED_OUTPUT_BYTES, DEFAULT_TRANSIENT_DIR_NAMES, OutputHygieneSpec,
    PostBuildHookSpec, ReportingOutputsSpec, ReportingSpec, SecretMaskingSpec,
};
pub use secrets::{SecretSpec, SecretsSpec, install_secrets};
pub use selection::{ImportSourceSpec, SelectionSpec};
pub use source::{
    ArchiveSourceSpec, DownloadSourceSpec, GitBackendSpec, GitSourceSpec, PathSourceSpec,
    SourceDefinition, SourcePinPolicySpec, SourceProviderKind, SourceRef, SourceRefreshPolicySpec,
    SourceSpec,
};
pub use source_dirs::{
    source_build_dir, source_build_dir_by_id, source_checkout_dir, source_materialized_dir,
};
pub use stage::StageSpec;
pub use stage::{StageContentOriginSpec, StageEnvSetSpec, StageFileSpec, StageServiceSpec};
pub use state::{IMAGE_ASSEMBLY_STATE_FILE_NAME, IMAGE_ASSEMBLY_STATE_KIND, KeyValueState};
pub use suggest::{did_you_mean_hint, edit_distance};
pub use target_arch::{TargetArch, target_arch};
pub use toolchain::{
    ResolvedToolchain, ToolchainKindSpec, ToolchainSpec, default_toolchain_prefix,
    resolve_toolchain,
};
pub use user_cache::{
    USER_BUILDROOT_CCACHE_DIR, USER_BUILDROOT_PACKAGE_CACHE_DIR, user_cache_root,
};
pub(crate) use variant::variant_name_suffix;
pub use variant::{DEFAULT_VARIANT_ID, VariantSpec, is_valid_variant_id, runtime_state_dir_name};
pub use workspace::WorkspacePathError;
pub use workspace::{
    CleanPolicy, WorkspaceNamedPathSpec, WorkspacePathKindSpec, WorkspaceSpec,
    default_image_collect_dir, resolve_workspace_path,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ResolvedBuildSpec {
    pub identity: BuildIdentity,
    pub selection: SelectionSpec,
    pub metadata: BuildMetadataSpec,
    pub inputs: InputSpec,
    pub policy: BuildPolicySpec,
    pub clean: CleanSpec,
    pub provenance: ProvenanceSpec,
    pub workspace: WorkspaceSpec,
    pub export: ExportSpec,
    pub sources: Vec<SourceSpec>,
    /// `[[toolchains]]`, in config order. Not serialized when empty, so a
    /// build without toolchains keeps its fingerprints.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub toolchains: Vec<ToolchainSpec>,
    pub artifacts: Vec<ArtifactSpec>,
    pub install: InstallSpec,
    pub stage: StageSpec,
    pub image: ImageSpec,
    pub checkpoints: CheckpointSpec,
    pub reporting: ReportingSpec,
    pub secrets: SecretsSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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
            export: ExportSpec::default(),
            sources: Vec::new(),
            toolchains: Vec::new(),
            artifacts: Vec::new(),
            install: InstallSpec::default(),
            stage: StageSpec::default(),
            image: ImageSpec::new(ImageDefinition::Buildroot(BuildrootImageSpec::default())),
            checkpoints: CheckpointSpec::default(),
            reporting: ReportingSpec::default(),
            secrets: SecretsSpec::default(),
        }
    }

    pub fn build_name(&self) -> &str {
        &self.identity.build_name
    }

    /// The name of the build's reuse state file (without `.reuse-state`):
    /// the build name, with `@<variant>` for a variant.
    pub fn state_name(&self) -> String {
        format!(
            "{}{}",
            self.identity.build_name,
            variant_name_suffix(self.selection.variant.as_deref())
        )
    }

    /// The name report files are written under: the build name, with
    /// `@<variant>` when the spec builds a variant.
    pub fn report_name(&self) -> String {
        format!(
            "{}{}",
            self.identity.build_name,
            variant_name_suffix(self.selection.variant.as_deref())
        )
    }

    pub fn display_name(&self) -> &str {
        &self.identity.display_name
    }
}
