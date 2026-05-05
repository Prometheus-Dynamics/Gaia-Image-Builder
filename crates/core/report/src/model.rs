use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportBundle {
    pub summary: RunSummary,
    pub selection: SelectionReport,
    pub provenance: ProvenanceReport,
    pub manifest: ManifestReport,
    pub rebuild_reasons: Vec<RebuildReasonReport>,
    pub execution_failures: Vec<ExecutionFailureReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunSummary {
    pub build_name: String,
    pub build_version: Option<String>,
    pub build_description: Option<String>,
    pub build_branch: Option<String>,
    pub build_target: Option<String>,
    pub build_profile: Option<String>,
    pub primary_image_output: Option<String>,
    pub operation_count: usize,
    pub warning_count: usize,
    pub error_count: usize,
    pub completed_operations: usize,
    pub reused_operations: usize,
    pub image_reused: bool,
    pub image_reuse_details: Vec<String>,
    pub rolled_back_operations: usize,
    pub cleanup_failure_count: usize,
    pub source_count: usize,
    pub artifact_count: usize,
    pub install_count: usize,
    pub stage_file_count: usize,
    pub stage_env_set_count: usize,
    pub stage_service_count: usize,
    pub checkpoint_count: usize,
    pub checkpoint_built_count: usize,
    pub checkpoint_reused_count: usize,
    pub rollback_on_error: bool,
    pub preserve_failed_outputs: bool,
    pub rollback_domains: Vec<String>,
    pub failure_classes: Vec<FailureClassCount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FailureClassCount {
    pub class: FailureClass,
    pub count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureClass {
    MissingSpec,
    MissingProvider,
    ToolStart,
    Timeout,
    Cancelled,
    OutputMissing,
    BackendCommand,
    PolicyBlocked,
    RuntimeState,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExecutionFailureReport {
    pub operation_id: String,
    pub code: String,
    pub class: FailureClass,
    pub message: String,
    pub output_tail: Vec<String>,
    pub cleanup_domain: Option<String>,
    pub cleanup_paths: Vec<String>,
    pub cleanup_status: CleanupStatus,
    pub cleanup_failures: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CleanupStatus {
    NotRequired,
    Cleaned,
    Preserved,
    DomainDisabled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProvenanceReport {
    pub build_name: String,
    pub build_version: Option<String>,
    pub build_branch: Option<String>,
    pub build_target: Option<String>,
    pub build_profile: Option<String>,
    pub selected_build_file: Option<String>,
    pub selected_preset: Option<String>,
    pub selected_inputs: Vec<(String, String)>,
    pub selected_env_files: Vec<String>,
    pub selected_env_overrides: Vec<(String, String)>,
    pub precedence_order: Vec<String>,
    pub precedence_layers: Vec<PrecedenceLayerReport>,
    pub explicit_overrides: Vec<(String, String)>,
    pub metadata_labels: Vec<(String, String)>,
    pub product_family: Option<String>,
    pub product_name: Option<String>,
    pub product_sku: Option<String>,
    pub identity_project: Option<String>,
    pub identity_vendor: Option<String>,
    pub identity_channel: Option<String>,
    pub identity_labels: Vec<(String, String)>,
    pub rollback_on_error: bool,
    pub preserve_failed_outputs: bool,
    pub rollback_domains: Vec<String>,
    pub source_providers: Vec<String>,
    pub artifact_providers: Vec<String>,
    pub artifact_install_identities: Vec<ArtifactInstallIdentityRecord>,
    pub artifact_output_metadata: Vec<ArtifactOutputMetadataRecord>,
    pub image_provider: String,
    pub image_feed_install_entries: Vec<String>,
    pub image_feed_stage_files: Vec<String>,
    pub image_feed_stage_env_sets: Vec<String>,
    pub image_feed_stage_services: Vec<String>,
    pub image_contract: BTreeMap<String, String>,
    pub image_output_collect_dirs: Vec<String>,
    pub image_output_archives: Vec<String>,
    pub output_hygiene_warnings: Vec<OutputHygieneWarningRecord>,
    pub source_backend_states: Vec<BackendStateRecord>,
    pub artifact_backend_states: Vec<BackendStateRecord>,
    pub image_backend_states: Vec<BackendStateRecord>,
    pub install_backend_states: Vec<BackendStateRecord>,
    pub stage_file_backend_states: Vec<BackendStateRecord>,
    pub stage_env_set_backend_states: Vec<BackendStateRecord>,
    pub stage_service_backend_states: Vec<BackendStateRecord>,
    pub image_assembly_backend_states: Vec<BackendStateRecord>,
    pub checkpoint_backend_states: Vec<BackendStateRecord>,
    pub completed_operation_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SelectionReport {
    pub requested_build: Option<String>,
    pub selected_build_file: Option<String>,
    pub selected_preset: Option<String>,
    pub selected_inputs: Vec<(String, String)>,
    pub selected_env_files: Vec<String>,
    pub selected_env_overrides: Vec<(String, String)>,
    pub explicit_overrides: Vec<(String, String)>,
    pub rollback_on_error: bool,
    pub preserve_failed_outputs: bool,
    pub rollback_domains: Vec<String>,
    pub precedence_order: Vec<String>,
    pub precedence_layers: Vec<PrecedenceLayerReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestReport {
    pub rollback_on_error: bool,
    pub preserve_failed_outputs: bool,
    pub rollback_domains: Vec<String>,
    pub image_feed_install_entries: Vec<String>,
    pub image_feed_stage_files: Vec<String>,
    pub image_feed_stage_env_sets: Vec<String>,
    pub image_feed_stage_services: Vec<String>,
    pub image_contract: BTreeMap<String, String>,
    pub operations: Vec<ManifestOperationRecord>,
    pub sources: Vec<ManifestSourceRecord>,
    pub artifacts: Vec<ManifestArtifactRecord>,
    pub installs: Vec<ManifestInstallRecord>,
    pub stage_files: Vec<ManifestStageFileRecord>,
    pub stage_env_sets: Vec<ManifestStageEnvSetRecord>,
    pub stage_services: Vec<ManifestStageServiceRecord>,
    pub image_outputs: Vec<ManifestImageOutputRecord>,
    pub image_assembly: Vec<BackendStateRecord>,
    pub output_hygiene_warnings: Vec<OutputHygieneWarningRecord>,
    pub checkpoints: Vec<ManifestCheckpointRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutputHygieneWarningRecord {
    pub code: String,
    pub directory: String,
    pub path: String,
    pub message: String,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackendStateRecord {
    pub id: String,
    pub state: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestOperationRecord {
    pub id: String,
    pub dependency_ids: Vec<String>,
    pub optionality: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestSourceRecord {
    pub id: String,
    pub provider: String,
    pub backend_state: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestArtifactRecord {
    pub id: String,
    pub provider: String,
    pub output_path: String,
    pub resolved_identifier_kind: Option<String>,
    pub resolved_identifier: Option<String>,
    pub produced_filename: Option<String>,
    pub output_class: Option<String>,
    pub build_tool: Option<String>,
    pub build_tool_version: Option<String>,
    pub install_name: Option<String>,
    pub install_class: Option<String>,
    pub install_destination_hint: Option<String>,
    pub backend_state: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtifactOutputMetadataRecord {
    pub artifact_id: String,
    pub provider: String,
    pub output_path: String,
    pub resolved_identifier_kind: Option<String>,
    pub resolved_identifier: Option<String>,
    pub produced_filename: Option<String>,
    pub output_class: Option<String>,
    pub build_tool: Option<String>,
    pub build_tool_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtifactInstallIdentityRecord {
    pub artifact_id: String,
    pub install_name: String,
    pub install_class: String,
    pub destination_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestInstallRecord {
    pub id: String,
    pub artifact_id: String,
    pub dest: String,
    pub backend_state: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestStageFileRecord {
    pub id: String,
    pub src: String,
    pub dest: String,
    pub origin: String,
    pub backend_state: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestStageEnvSetRecord {
    pub id: String,
    pub name: String,
    pub entry_count: usize,
    pub backend_state: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestStageServiceRecord {
    pub id: String,
    pub name: String,
    pub unit_path: String,
    pub backend_state: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestImageOutputRecord {
    pub provider_id: String,
    pub image_contract: BTreeMap<String, String>,
    pub image_feed_install_entries: Vec<String>,
    pub image_feed_stage_files: Vec<String>,
    pub image_feed_stage_env_sets: Vec<String>,
    pub image_feed_stage_services: Vec<String>,
    pub collect_dir: Option<String>,
    pub archive_path: Option<String>,
    pub emit_report: bool,
    pub reused: bool,
    pub reuse_details: Vec<String>,
    pub backend_state: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestCheckpointRecord {
    pub id: String,
    pub backend: Option<String>,
    pub anchor: String,
    pub backend_state: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RebuildReasonReport {
    pub operation_id: String,
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrecedenceLayerReport {
    pub source: String,
    pub applies_to: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportOutputBundle {
    pub files: Vec<ReportOutputFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportOutputFile {
    pub kind: ReportFileKind,
    pub path: PathBuf,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFileKind {
    Summary,
    Selection,
    Provenance,
    Manifest,
    RebuildReasons,
}

impl ReportFileKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Selection => "selection",
            Self::Provenance => "provenance",
            Self::Manifest => "manifest",
            Self::RebuildReasons => "rebuild-reasons",
        }
    }
}
