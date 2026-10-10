mod content_digests;
mod materialize;
mod preview;
pub use content_digests::{
    COLLECT_STATE_FILE, CONTENT_DIGESTS_FILE, TEMP_FILE_SUFFIX, collect_content_files,
    is_content_walk_excluded, record_collect_dir_digests, record_content_digests, recorded_sha256,
    sha256_hex,
};
pub use gaia_process::{
    ProcessCancelCheck, ProcessLogLine, ProcessLogSink, ProcessOutputRetention,
};
use gaia_spec::{
    BuildrootOverrideCheckSpec, ImageDefinition, ImageProviderKind, ImageSpec, ResolvedBuildSpec,
    RetryBackoffStrategySpec,
};
pub use materialize::{finalize_temp_image_output, materialize_image_output};
pub use preview::{
    ImagePreview, PreviewCleanKind, PreviewDeletion, PreviewDeletionKind, PreviewSection,
};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

pub trait ImageProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn kind(&self) -> ImageProviderKind;
    fn supports(&self, _spec: &ResolvedBuildSpec) -> bool {
        true
    }
    fn plan_image(&self, _image: &ImageSpec) -> ImagePlan {
        ImagePlan {
            operations: vec![ImageProviderOperation::Build],
            output: ImageOutputContract::default(),
        }
    }
    fn validate_image(&self, _image: &ImageSpec) -> Vec<ImageProviderValidationIssue> {
        Vec::new()
    }
    /// What running `operation` would do to this provider's state, changing
    /// nothing. `None` when the provider has no preview.
    fn preview_image(
        &self,
        _spec: &ResolvedBuildSpec,
        _image: &ImageSpec,
        _policy: &ImageExecutionPolicy,
        _operation: ImageProviderOperation,
    ) -> Result<Option<ImagePreview>, ImageProviderError> {
        Ok(None)
    }
    fn execute_image(
        &self,
        _spec: &ResolvedBuildSpec,
        image: &ImageSpec,
        _output: &ImageOutputContract,
        _policy: &ImageExecutionPolicy,
        _log_sink: Option<ProcessLogSink>,
        _cancel_check: Option<ProcessCancelCheck>,
    ) -> Result<ImageExecutionResult, ImageProviderError> {
        Err(ImageProviderError::new(
            ImageProviderErrorKind::PolicyBlocked,
            format!(
                "image provider '{}' must implement execute_image for {:?}",
                self.id(),
                image.provider_kind(),
            ),
        ))
    }

    fn execute_image_operation(
        &self,
        request: ImageOperationExecution<'_>,
    ) -> Result<ImageExecutionResult, ImageProviderError> {
        match request.operation {
            ImageProviderOperation::Build => self.execute_image(
                request.spec,
                request.image,
                request.output,
                request.policy,
                request.log_sink,
                request.cancel_check,
            ),
            ImageProviderOperation::Prepare => Err(ImageProviderError::new(
                ImageProviderErrorKind::PolicyBlocked,
                format!(
                    "image provider '{}' does not support prepare/finalize split execution",
                    self.id()
                ),
            )),
        }
    }
}

pub struct ImageOperationExecution<'a> {
    pub spec: &'a ResolvedBuildSpec,
    pub image: &'a ImageSpec,
    pub operation: ImageProviderOperation,
    pub output: &'a ImageOutputContract,
    pub policy: &'a ImageExecutionPolicy,
    pub log_sink: Option<ProcessLogSink>,
    pub cancel_check: Option<ProcessCancelCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagePlan {
    pub operations: Vec<ImageProviderOperation>,
    pub output: ImageOutputContract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProviderOperation {
    Prepare,
    Build,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageProviderValidationIssue {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProviderErrorKind {
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
pub struct ImageProviderError {
    pub kind: ImageProviderErrorKind,
    pub message: String,
    /// Step time messages (`gaia_process::step_time_message`) of the work
    /// done before the failure, so a failed or cancelled operation still
    /// shows where its time went.
    pub step_times: Vec<String>,
}

impl ImageProviderError {
    pub fn new(kind: ImageProviderErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            step_times: Vec::new(),
        }
    }

    pub fn with_step_times(mut self, step_times: impl IntoIterator<Item = String>) -> Self {
        self.step_times.extend(step_times);
        self
    }

    pub fn backend_command(message: impl Into<String>) -> Self {
        Self::new(ImageProviderErrorKind::BackendCommand, message)
    }

    pub fn output_missing(message: impl Into<String>) -> Self {
        Self::new(ImageProviderErrorKind::OutputMissing, message)
    }

    pub fn runtime_state(message: impl Into<String>) -> Self {
        Self::new(ImageProviderErrorKind::RuntimeState, message)
    }
}

impl From<String> for ImageProviderError {
    fn from(value: String) -> Self {
        Self::new(ImageProviderErrorKind::Unknown, value)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageOutputContract {
    pub collect_dir: Option<String>,
    pub archive_name: Option<String>,
    pub emit_report: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageExecutionPolicy {
    pub retry_attempts: u32,
    pub retry_backoff_ms: u64,
    pub retry_backoff_strategy: RetryBackoffStrategySpec,
    pub timeout_seconds: u64,
    pub jobs: u32,
    pub local_jobs: u32,
    pub download_dir: Option<String>,
    pub ccache_enabled: bool,
    pub ccache_dir: Option<String>,
    /// ccache `max_size`, such as `50G`.
    pub ccache_max_size: Option<String>,
    /// Buildroot: build independent packages concurrently (per-package
    /// directories and a top-level `make -j`).
    pub parallel_packages: bool,
    /// Buildroot: reuse built packages from a cache shared by every build.
    pub package_cache: gaia_spec::BuildrootPackageCachePolicySpec,
    /// Buildroot: where the output tree is built (disk, ram, or a directory).
    pub work_dir: gaia_spec::BuildrootWorkDirPolicySpec,
    /// Buildroot: where each host tool (ccache, pkgconf) comes from.
    pub host_tools: gaia_spec::BuildrootHostToolsPolicySpec,
    /// Buildroot: share one compiled output tree between builds whose
    /// Buildroot inputs are identical.
    pub shared_output: bool,
    /// Buildroot: root directory for shared output trees.
    pub shared_output_dir: Option<String>,
    /// Buildroot: what to do when a requested `config_overrides` entry is
    /// missing or different in the final `.config`.
    pub override_check: BuildrootOverrideCheckSpec,
    pub kernel_modules_check: BuildrootOverrideCheckSpec,
    pub output_retention: ProcessOutputRetention,
}

impl Default for ImageExecutionPolicy {
    fn default() -> Self {
        Self {
            retry_attempts: 1,
            retry_backoff_ms: 0,
            retry_backoff_strategy: RetryBackoffStrategySpec::Fixed,
            timeout_seconds: 300,
            jobs: 0,
            local_jobs: 0,
            download_dir: None,
            ccache_enabled: false,
            ccache_dir: None,
            ccache_max_size: None,
            parallel_packages: false,
            package_cache: gaia_spec::BuildrootPackageCachePolicySpec::default(),
            work_dir: gaia_spec::BuildrootWorkDirPolicySpec::default(),
            host_tools: gaia_spec::BuildrootHostToolsPolicySpec::default(),
            shared_output: false,
            shared_output_dir: None,
            override_check: BuildrootOverrideCheckSpec::default(),
            kernel_modules_check: BuildrootOverrideCheckSpec::default(),
            output_retention: ProcessOutputRetention::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageExecutionResult {
    pub provider_id: String,
    pub collect_dir: Option<PathBuf>,
    pub archive_path: Option<PathBuf>,
    pub emit_report: bool,
    pub reused: bool,
    pub reuse_details: Vec<String>,
    pub messages: Vec<String>,
    /// Problems worth surfacing in the run output and report even though the
    /// operation succeeded.
    pub warnings: Vec<String>,
    /// Facts worth a line in the run summary, such as the compiler cache
    /// hit rate.
    pub notes: Vec<String>,
    pub state_details: Vec<(String, String)>,
}

pub fn build_state_details(spec: &ResolvedBuildSpec) -> Vec<(String, String)> {
    vec![
        (
            "execution_backend".to_string(),
            if spec.policy.execution.docker.is_some() {
                "docker".to_string()
            } else {
                "host".to_string()
            },
        ),
        (
            "execution_backend_image".to_string(),
            spec.policy
                .execution
                .docker
                .as_ref()
                .map(|docker| docker.image.clone())
                .unwrap_or_default(),
        ),
        (
            "build_version".to_string(),
            spec.identity.version.clone().unwrap_or_default(),
        ),
        (
            "build_branch".to_string(),
            spec.metadata.branch.clone().unwrap_or_default(),
        ),
        (
            "build_target".to_string(),
            spec.metadata.target.clone().unwrap_or_default(),
        ),
        (
            "build_profile".to_string(),
            spec.metadata.profile.clone().unwrap_or_default(),
        ),
    ]
}

pub fn build_image_contract_state_details(image: &ImageSpec) -> Vec<(String, String)> {
    let mut details = vec![
        (
            "feed_install_entries".to_string(),
            join_ids(image.feed.install_entries.iter().map(|id| id.as_str())),
        ),
        (
            "feed_stage_files".to_string(),
            join_ids(image.feed.stage_files.iter().map(|id| id.as_str())),
        ),
        (
            "feed_stage_env_sets".to_string(),
            join_ids(image.feed.stage_env_sets.iter().map(|id| id.as_str())),
        ),
        (
            "feed_stage_services".to_string(),
            join_ids(image.feed.stage_services.iter().map(|id| id.as_str())),
        ),
    ];

    match &image.definition {
        ImageDefinition::Buildroot(buildroot) => {
            details.push((
                "buildroot_external_tree_mode".to_string(),
                buildroot.external_tree_mode.as_str().to_string(),
            ));
            details.push((
                "buildroot_expected_images".to_string(),
                buildroot
                    .expected_images
                    .iter()
                    .map(|image| {
                        format!(
                            "{}:{}:{}",
                            image.name,
                            image.format.as_str(),
                            if image.required {
                                "required"
                            } else {
                                "optional"
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(","),
            ));
        }
        ImageDefinition::StartingPoint(starting_point) => {
            details.push((
                "starting_point_rootfs_validation_mode".to_string(),
                starting_point.rootfs_validation_mode.as_str().to_string(),
            ));
            details.push((
                "starting_point_output_mode".to_string(),
                starting_point.output_mode.as_str().to_string(),
            ));
        }
    }

    details
}

fn join_ids<'a>(ids: impl Iterator<Item = &'a str>) -> String {
    ids.collect::<Vec<_>>().join(",")
}

pub fn file_sha256_or_placeholder(path: &Path) -> String {
    let output = Command::new("sha256sum").arg(path).output().ok();
    let Some(output) = output else {
        return format!("sha256-unavailable:{}", path.display());
    };
    if !output.status.success() {
        return format!(
            "sha256-error:{}:{}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string()
}

pub fn path_bytes(path: &Path) -> u64 {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

pub fn dir_digest(path: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    hash_dir(path, &mut hasher);
    format!("{:016x}", hasher.finish())
}

fn hash_dir(path: &Path, hasher: &mut DefaultHasher) {
    path.display().to_string().hash(hasher);
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => {
            "missing".hash(hasher);
            return;
        }
    };
    metadata.is_dir().hash(hasher);
    metadata.is_file().hash(hasher);
    metadata.len().hash(hasher);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode().hash(hasher);
    }
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
        .hash(hasher);
    if metadata.is_dir() {
        let mut entries = match fs::read_dir(path) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect::<Vec<_>>(),
            Err(_) => return,
        };
        entries.sort();
        for entry in entries {
            hash_dir(&entry, hasher);
        }
    }
}

pub fn temporary_publish_output_path(output: &Path, fallback_name: &str) -> PathBuf {
    let Some(parent) = output.parent() else {
        return output.with_extension("gaia-tmp");
    };
    let file_name = output
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(fallback_name);
    parent.join(format!(".{file_name}.gaia-tmp"))
}

pub fn temporary_publish_backup_path(output: &Path, fallback_name: &str) -> PathBuf {
    let Some(parent) = output.parent() else {
        return output.with_extension("gaia-backup");
    };
    let file_name = output
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(fallback_name);
    parent.join(format!(".{file_name}.gaia-backup"))
}

pub fn publish_replace_output(
    temp: &Path,
    output: &Path,
    output_label: &str,
    fallback_name: &str,
) -> Result<(), String> {
    match fs::rename(temp, output) {
        Ok(()) => return Ok(()),
        Err(error) if !output.exists() => {
            return Err(format!(
                "failed to publish {output_label} '{}' from '{}': {error}",
                output.display(),
                temp.display()
            ));
        }
        Err(_) => {}
    }

    let backup = temporary_publish_backup_path(output, fallback_name);
    if backup.exists() {
        fs::remove_file(&backup).map_err(|error| {
            format!(
                "failed to remove stale {output_label} backup '{}': {error}",
                backup.display()
            )
        })?;
    }
    fs::rename(output, &backup).map_err(|error| {
        format!(
            "failed to move existing {output_label} '{}' to backup '{}': {error}",
            output.display(),
            backup.display()
        )
    })?;
    match fs::rename(temp, output) {
        Ok(()) => {
            let _ = fs::remove_file(&backup);
            Ok(())
        }
        Err(error) => {
            let restore_result = fs::rename(&backup, output);
            let restore_message = match restore_result {
                Ok(()) => format!("previous {output_label} was restored"),
                Err(restore_error) => format!(
                    "failed to restore previous {output_label} from '{}': {restore_error}",
                    backup.display()
                ),
            };
            Err(format!(
                "failed to publish {output_label} '{}' from '{}': {error}; {restore_message}",
                output.display(),
                temp.display()
            ))
        }
    }
}

#[derive(Default)]
pub struct ImageProviderCatalog {
    providers: Vec<Box<dyn ImageProvider>>,
}

impl ImageProviderCatalog {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    pub fn register(&mut self, provider: Box<dyn ImageProvider>) {
        self.providers.push(provider);
    }

    pub fn find_for_kind(&self, kind: ImageProviderKind) -> Option<&dyn ImageProvider> {
        self.providers
            .iter()
            .map(Box::as_ref)
            .find(|provider| provider.kind() == kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_spec::BuildrootImageSpec;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(prefix: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir()
            .join("gaia-tests")
            .join(format!("{prefix}-{nonce}"))
    }

    struct DummyImageProvider;

    impl ImageProvider for DummyImageProvider {
        fn id(&self) -> &'static str {
            "image.dummy"
        }

        fn kind(&self) -> ImageProviderKind {
            ImageProviderKind::Buildroot
        }
    }

    #[test]
    fn default_image_execution_fails_instead_of_materializing_placeholder() {
        let spec = ResolvedBuildSpec::new("default-image-exec");
        let image = ImageSpec::new(ImageDefinition::Buildroot(BuildrootImageSpec::default()));
        let output = ImageOutputContract {
            collect_dir: Some(temp_path("gaia-image-default-exec").display().to_string()),
            archive_name: Some("image.tar".into()),
            emit_report: false,
        };

        let error = DummyImageProvider
            .execute_image(
                &spec,
                &image,
                &output,
                &ImageExecutionPolicy::default(),
                None,
                None,
            )
            .expect_err("default image execution should fail");

        assert_eq!(error.kind, ImageProviderErrorKind::PolicyBlocked);
        assert!(!PathBuf::from(output.collect_dir.expect("collect dir")).exists());
    }

    #[test]
    fn materialize_image_output_cleans_temp_archive_when_rename_fails() {
        let root = temp_path("gaia-image-output-failure");
        fs::create_dir_all(&root).expect("root dir");
        let archive_path = root.join("existing-dir");
        fs::create_dir_all(&archive_path).expect("existing archive dir");
        let result = ImageExecutionResult {
            provider_id: "image.test".into(),
            collect_dir: None,
            archive_path: Some(archive_path.clone()),
            emit_report: true,
            reused: false,
            reuse_details: Vec::new(),
            messages: Vec::new(),
            warnings: Vec::new(),
            notes: Vec::new(),
            state_details: Vec::new(),
        };

        let error = materialize_image_output(&result)
            .expect_err("rename into existing directory should fail");

        assert!(error.message.contains("failed to move image archive"));
        assert!(!archive_path.with_extension("gaia.tmp").exists());
    }

    #[test]
    fn publish_replace_output_restores_previous_output_when_replacement_fails() {
        let root = temp_path("gaia-publish-replace-restore");
        let output = root.join("output.img");
        let missing_temp = temporary_publish_output_path(&output, "output");
        fs::create_dir_all(&root).expect("root dir");
        fs::write(&output, "previous").expect("previous output");

        let error = publish_replace_output(&missing_temp, &output, "test output", "output")
            .expect_err("publish failure");

        assert!(
            error.contains("previous test output was restored"),
            "{error}"
        );
        assert_eq!(
            fs::read_to_string(&output).expect("restored output"),
            "previous"
        );
        assert!(!temporary_publish_backup_path(&output, "output").exists());
    }

    #[test]
    fn a_provider_without_a_preview_says_so_instead_of_failing() {
        let spec = ResolvedBuildSpec::new("preview-default");
        let image = ImageSpec {
            definition: ImageDefinition::Buildroot(BuildrootImageSpec::default()),
            feed: gaia_spec::ImageFeedSpec::default(),
            output: gaia_spec::ImageOutputSpec::default(),
            assembly: None,
        };
        let preview = DummyImageProvider.preview_image(
            &spec,
            &image,
            &ImageExecutionPolicy::default(),
            ImageProviderOperation::Build,
        );
        assert_eq!(preview.expect("no error"), None);
    }

    #[test]
    fn preview_deletions_outside_trash_decide_fail_on_clean() {
        let deletion = |kind| PreviewDeletion {
            kind,
            path: "/x".into(),
            reason: "test".into(),
        };
        let mut preview = ImagePreview {
            provider_id: "image.test".into(),
            sections: Vec::new(),
            clean: PreviewCleanKind::Nothing,
            clean_reasons: Vec::new(),
            rebuilt_packages: Vec::new(),
            uninstalled_packages: Vec::new(),
            deletions: vec![deletion(PreviewDeletionKind::Trash)],
            blocked: None,
            verdict: String::new(),
        };
        // Leftovers of an earlier clean are purged either way.
        assert_eq!(preview.deletions_outside_trash(), 0);
        assert!(!preview.trips_fail_on_clean());
        preview.deletions.push(deletion(PreviewDeletionKind::Cache));
        assert!(preview.trips_fail_on_clean());
        preview.deletions.clear();
        preview.clean = PreviewCleanKind::Full;
        assert!(preview.trips_fail_on_clean());
        assert_eq!(PreviewCleanKind::Packages.as_str(), "packages");
    }
}
