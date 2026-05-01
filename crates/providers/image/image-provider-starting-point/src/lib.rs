use gaia_image_providers::{
    ImageExecutionPolicy, ImageExecutionResult, ImageOutputContract, ImagePlan, ImageProvider,
    ImageProviderError, ImageProviderErrorKind, ImageProviderOperation,
    ImageProviderValidationIssue, ProcessCancelCheck, ProcessLogSink, ProcessOutputRetention,
    build_image_contract_state_details, build_state_details, dir_digest,
    file_sha256_or_placeholder, materialize_image_output,
};
use gaia_process::{
    DockerRunSpec, ProcessRetryBackoffStrategy, ProcessRunErrorKind, docker_run_command,
    label_process_log_sink, retry_backoff_duration as process_retry_backoff_duration,
    run_command_with_timeout, run_command_with_timeout_and_retention, sleep_with_cancel,
};
use gaia_spec::{
    ImageDefinition, ImageSpec, ResolvedBuildSpec, RetryBackoffStrategySpec, SourceId,
    StartingPointOutputModeSpec, StartingPointPackagesSpec, StartingPointRootfsValidationModeSpec,
};
use std::collections::BTreeMap;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

const PRIVILEGED_CLEANUP_TIMEOUT_SECONDS: u64 = 30;

pub struct StartingPointImageProvider;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImageExecutionContext {
    workspace_root: PathBuf,
    docker_image: Option<String>,
}

impl ImageProvider for StartingPointImageProvider {
    fn id(&self) -> &'static str {
        "image.starting-point"
    }

    fn kind(&self) -> gaia_spec::ImageProviderKind {
        gaia_spec::ImageProviderKind::StartingPoint
    }

    fn supports(&self, _spec: &ResolvedBuildSpec) -> bool {
        true
    }

    fn plan_image(&self, image: &ImageSpec) -> ImagePlan {
        let output = ImageOutputContract {
            collect_dir: image.output.collect_dir.clone(),
            archive_name: image.output.archive_name.clone(),
            emit_report: image.output.emit_report,
        };
        ImagePlan {
            operations: vec![ImageProviderOperation::Build],
            output,
        }
    }

    fn validate_image(&self, image: &ImageSpec) -> Vec<ImageProviderValidationIssue> {
        let mut issues = Vec::new();
        if let ImageDefinition::StartingPoint(starting_point) = &image.definition
            && starting_point.source.is_none()
            && !starting_point.rootfs_path.starts_with('/')
        {
            issues.push(ImageProviderValidationIssue {
                code: "starting_point_rootfs_not_absolute",
                message: "starting-point rootfs_path should be absolute".into(),
            });
        }
        if let ImageDefinition::StartingPoint(starting_point) = &image.definition
            && starting_point.output_mode == StartingPointOutputModeSpec::ArchiveOnly
            && image.output.archive_name.is_none()
        {
            issues.push(ImageProviderValidationIssue {
                code: "starting_point_archive_without_archive_name",
                message: "starting-point archive-only mode requires archive_name".into(),
            });
        }
        issues
    }

    fn execute_image(
        &self,
        spec: &ResolvedBuildSpec,
        image: &ImageSpec,
        output: &ImageOutputContract,
        policy: &ImageExecutionPolicy,
        log_sink: Option<ProcessLogSink>,
        cancel_check: Option<ProcessCancelCheck>,
    ) -> Result<ImageExecutionResult, ImageProviderError> {
        let execution = execution_context(spec);
        let (rootfs, rootfs_label, rootfs_kind_label, rootfs_source) = match &image.definition {
            ImageDefinition::StartingPoint(starting_point) => {
                let rootfs = resolve_starting_point_rootfs(spec, starting_point)?;
                let label = if let Some(source_id) = &starting_point.source {
                    match &starting_point.source_path {
                        Some(path) if !path.trim().is_empty() => {
                            format!("source:{}:{}", source_id.as_str(), path)
                        }
                        _ => format!("source:{}", source_id.as_str()),
                    }
                } else {
                    starting_point.rootfs_path.clone()
                };
                let source = starting_point
                    .source
                    .as_ref()
                    .map(|source_id| source_id.as_str().to_string())
                    .unwrap_or_else(|| "direct-path".to_string());
                let kind = if rootfs.is_dir() { "directory" } else { "file" }.to_string();
                (rootfs, label, kind, source)
            }
            _ => (
                PathBuf::new(),
                String::new(),
                "file".to_string(),
                "direct-path".to_string(),
            ),
        };
        let collect_dir = output
            .collect_dir
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("out/images/starting-point"));
        let archive_path = output
            .archive_name
            .as_ref()
            .map(|archive_name| collect_dir.join(archive_name));

        let (validation_mode, output_mode) = match &image.definition {
            ImageDefinition::StartingPoint(starting_point) => (
                starting_point.rootfs_validation_mode,
                starting_point.output_mode,
            ),
            _ => (
                StartingPointRootfsValidationModeSpec::RequireExists,
                StartingPointOutputModeSpec::CopyAndArchive,
            ),
        };
        let mut state_details = vec![
            ("rootfs_path".to_string(), rootfs_label.clone()),
            ("rootfs_kind".to_string(), rootfs_kind_label),
            ("rootfs_source".to_string(), rootfs_source),
        ];
        let mut messages = Vec::new();
        validate_rootfs(&rootfs, validation_mode)?;
        if rootfs.exists() {
            if looks_like_raw_image(&rootfs) {
                let final_image_path = archive_path.clone().ok_or_else(|| {
                    ImageProviderError::new(
                        ImageProviderErrorKind::PolicyBlocked,
                        "starting-point raw image mutation requires image.output.archive_name"
                            .to_string(),
                    )
                })?;
                messages.extend(materialize_mutable_raw_image(MutableRawImageRequest {
                    spec,
                    image,
                    source_image: &rootfs,
                    collect_dir: &collect_dir,
                    final_image_path: &final_image_path,
                    policy,
                    log_sink: log_sink.clone(),
                    cancel_check: cancel_check.clone(),
                })?);
                state_details.push((
                    "rootfs_digest".to_string(),
                    file_sha256_or_placeholder(&final_image_path),
                ));
            } else {
                let mutable_rootfs = materialize_mutable_rootfs(MutableRootfsRequest {
                    spec,
                    image,
                    rootfs: &rootfs,
                    collect_dir: &collect_dir,
                    execution: &execution,
                    policy,
                    log_sink: log_sink.clone(),
                    cancel_check: cancel_check.clone(),
                    messages: &mut messages,
                })?;
                if let Some(archive_path) = &archive_path
                    && matches!(
                        output_mode,
                        StartingPointOutputModeSpec::ArchiveOnly
                            | StartingPointOutputModeSpec::CopyAndArchive
                    )
                {
                    messages.extend(create_rootfs_archive(
                        &mutable_rootfs,
                        archive_path,
                        &execution,
                        policy,
                        log_sink.clone(),
                        cancel_check.clone(),
                    )?);
                }
                state_details.push(("rootfs_digest".to_string(), dir_digest(&mutable_rootfs)));
            }
        }

        let result = ImageExecutionResult {
            provider_id: self.id().into(),
            collect_dir: Some(collect_dir),
            archive_path,
            emit_report: output.emit_report,
            reused: false,
            reuse_details: Vec::new(),
            messages: {
                messages.push(format!(
                    "starting-point image built from rootfs '{}'",
                    rootfs_label
                ));
                messages
            },
            state_details: {
                let mut details = state_details;
                details.extend(build_state_details(spec));
                details.extend(build_image_contract_state_details(image));
                details
            },
        };
        materialize_image_output(&result)?;
        Ok(result)
    }
}

mod archive_extract;
mod chroot;
mod command;
mod feed;
mod packages;
mod raw_image;
mod rootfs;
#[cfg(test)]
mod tests;

pub(crate) use archive_extract::*;
pub(crate) use chroot::*;
pub(crate) use command::*;
pub(crate) use feed::*;
pub(crate) use packages::*;
pub(crate) use raw_image::*;
pub(crate) use rootfs::*;
