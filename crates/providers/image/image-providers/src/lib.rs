pub use gaia_process::{
    ProcessCancelCheck, ProcessLogLine, ProcessLogSink, ProcessOutputRetention,
};
use gaia_spec::{
    ImageDefinition, ImageProviderKind, ImageSpec, ResolvedBuildSpec, RetryBackoffStrategySpec,
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
}

impl ImageProviderError {
    pub fn new(kind: ImageProviderErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageExecutionPolicy {
    pub retry_attempts: u32,
    pub retry_backoff_ms: u64,
    pub retry_backoff_strategy: RetryBackoffStrategySpec,
    pub timeout_seconds: u64,
    pub jobs: u32,
    pub local_jobs: u32,
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
    pub state_details: Vec<(String, String)>,
}

pub fn materialize_image_output(result: &ImageExecutionResult) -> Result<(), ImageProviderError> {
    if let Some(collect_dir) = &result.collect_dir {
        fs::create_dir_all(collect_dir)
            .map_err(|error| {
                format!(
                    "failed to create image collect dir '{}': {error}",
                    collect_dir.display()
                )
            })
            .map_err(ImageProviderError::backend_command)?;
        let marker = collect_dir.join("image-provider.txt");
        fs::write(
            &marker,
            format!(
                "provider={}\nemit_report={}\n",
                result.provider_id, result.emit_report
            ),
        )
        .map_err(|error| {
            format!(
                "failed to write image marker '{}': {error}",
                marker.display()
            )
        })
        .map_err(ImageProviderError::runtime_state)?;
        let state_path = collect_dir.join(".gaia-image-state.txt");
        fs::write(&state_path, render_image_state(result))
            .map_err(|error| {
                format!(
                    "failed to write image state '{}': {error}",
                    state_path.display()
                )
            })
            .map_err(ImageProviderError::backend_command)?;
    }
    if let Some(archive_path) = &result.archive_path {
        if archive_path.is_file() {
            return Ok(());
        }
        if let Some(parent) = archive_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| {
                    format!(
                        "failed to create archive dir '{}': {error}",
                        parent.display()
                    )
                })
                .map_err(ImageProviderError::backend_command)?;
        }
        let temp_archive = archive_path.with_extension("gaia.tmp");
        fs::write(
            &temp_archive,
            format!("provider={}\narchive=true\n", result.provider_id),
        )
        .map_err(|error| {
            format!(
                "failed to write image temp archive '{}': {error}",
                temp_archive.display()
            )
        })
        .map_err(ImageProviderError::backend_command)?;
        finalize_temp_image_output(&temp_archive, archive_path, "image archive")?;
    }
    Ok(())
}

pub fn finalize_temp_image_output(
    temp_output: &Path,
    output_path: &Path,
    label: &str,
) -> Result<(), ImageProviderError> {
    fs::rename(temp_output, output_path).map_err(|error| {
        let _ = fs::remove_file(temp_output);
        ImageProviderError::backend_command(format!(
            "failed to move {label} '{}' into place '{}': {error}",
            temp_output.display(),
            output_path.display()
        ))
    })
}

fn render_image_state(result: &ImageExecutionResult) -> String {
    let mut state = gaia_spec::KeyValueState::new()
        .with("provider", result.provider_id.as_str())
        .with("emit_report", result.emit_report)
        .with(
            "archive",
            result
                .archive_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        )
        .with("reused", result.reused);
    for (index, detail) in result.reuse_details.iter().enumerate() {
        state.insert(format!("reuse_detail_{index}"), detail);
    }
    if let Some(collect_dir) = &result.collect_dir {
        state.insert("collect_digest", dir_digest(collect_dir));
    }
    if let Some(archive_path) = &result.archive_path {
        state.insert("archive_sha256", file_sha256_or_placeholder(archive_path));
        state.insert("archive_bytes", path_bytes(archive_path));
    }
    state.extend_pairs(result.state_details.iter().cloned());
    state.render()
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
            state_details: Vec::new(),
        };

        let error = materialize_image_output(&result)
            .expect_err("rename into existing directory should fail");

        assert!(error.message.contains("failed to move image archive"));
        assert!(!archive_path.with_extension("gaia.tmp").exists());
    }
}
