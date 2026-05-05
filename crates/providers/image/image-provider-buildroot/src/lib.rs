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
    run_command_stdout_to_file_with_timeout_and_retention, run_command_with_timeout_and_retention,
    sleep_with_cancel,
};
use gaia_spec::{
    BuildrootExpectedImageFormatSpec, BuildrootExternalTreeModeSpec, ImageDefinition, ImageSpec,
    ResolvedBuildSpec, RetryBackoffStrategySpec, SourceId,
};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

pub struct BuildrootImageProvider;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImageExecutionContext {
    workspace_root: PathBuf,
    docker_image: Option<String>,
}

impl ImageProvider for BuildrootImageProvider {
    fn id(&self) -> &'static str {
        "image.buildroot"
    }

    fn kind(&self) -> gaia_spec::ImageProviderKind {
        gaia_spec::ImageProviderKind::Buildroot
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
        let operations = match &image.definition {
            ImageDefinition::Buildroot(buildroot) if buildroot.source.is_some() => {
                vec![
                    ImageProviderOperation::Prepare,
                    ImageProviderOperation::Build,
                ]
            }
            _ => vec![ImageProviderOperation::Build],
        };
        ImagePlan { operations, output }
    }

    fn validate_image(&self, image: &ImageSpec) -> Vec<ImageProviderValidationIssue> {
        let mut issues = Vec::new();
        if let ImageDefinition::Buildroot(buildroot) = &image.definition
            && let Some(external_tree) = &buildroot.external_tree
            && external_tree.trim().is_empty()
        {
            issues.push(ImageProviderValidationIssue {
                code: "buildroot_external_tree_empty",
                message: "buildroot external_tree cannot be empty when set".into(),
            });
        }
        if let ImageDefinition::Buildroot(buildroot) = &image.definition {
            if buildroot.external_tree_mode == BuildrootExternalTreeModeSpec::Required
                && buildroot.external_tree.is_none()
            {
                issues.push(ImageProviderValidationIssue {
                    code: "buildroot_external_tree_required",
                    message: "buildroot external_tree_mode=required requires external_tree".into(),
                });
            }
            if buildroot.external_tree_mode == BuildrootExternalTreeModeSpec::Disabled
                && buildroot.external_tree.is_some()
            {
                issues.push(ImageProviderValidationIssue {
                    code: "buildroot_external_tree_disabled",
                    message: "buildroot external_tree_mode=disabled does not allow external_tree"
                        .into(),
                });
            }
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
        let (defconfig, defconfig_path) = match &image.definition {
            ImageDefinition::Buildroot(buildroot) => (
                buildroot
                    .defconfig
                    .clone()
                    .unwrap_or_else(|| "default".into()),
                buildroot.defconfig_path.clone(),
            ),
            _ => ("default".into(), None),
        };
        let collect_dir = output
            .collect_dir
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("out/images/buildroot"));
        let archive_path = output
            .archive_name
            .as_ref()
            .map(|archive_name| collect_dir.join(archive_name));
        let execution = execution_context(spec);

        let mut messages = Vec::new();
        let mut reuse_details = Vec::new();
        let mut state_details = vec![("defconfig".to_string(), defconfig.clone())];
        if let Some(path) = &defconfig_path {
            state_details.push(("defconfig_path".to_string(), path.clone()));
        }
        if let Some(buildroot_dir) = resolve_buildroot_dir(spec, image) {
            let output_dir = collect_dir.join("buildroot-output");
            let target_dir = output_dir.join("target");
            messages.extend(run_buildroot(BuildrootRunRequest {
                spec,
                image,
                buildroot_dir: &buildroot_dir,
                output_dir: &output_dir,
                command: ImageCommandContext {
                    execution: &execution,
                    policy,
                    log_sink: log_sink.clone(),
                    cancel_check: cancel_check.clone(),
                },
            })?);
            if image_feed_has_content(image) {
                let feed_signature = build_image_feed_signature(spec, image)?;
                if buildroot_expected_images_present(image, &output_dir)
                    && image_feed_signature_is_current(&output_dir, &feed_signature)
                    && image_feed_outputs_present(spec, image, &target_dir)
                {
                    messages.push(format!(
                        "reused image feed overlay and refreshed images at '{}'",
                        output_dir.display()
                    ));
                    reuse_details.push("image-feed-overlay".to_string());
                } else {
                    apply_image_feed_to_rootfs(spec, image, &target_dir)?;
                    messages.extend(refresh_buildroot_images_after_feed_overlay(
                        image,
                        &buildroot_dir,
                        &output_dir,
                        &execution,
                        policy,
                        log_sink.clone(),
                        cancel_check.clone(),
                    )?);
                    refresh_expected_tar_images(image, &target_dir, &output_dir, &execution)?;
                    write_image_feed_signature(&output_dir, &feed_signature)?;
                }
            }
            let matched_expected_images =
                collect_expected_images(image, &output_dir, &collect_dir)?;
            if let Some(archive_path) = &archive_path {
                messages.extend(archive_buildroot_output(BuildrootArchiveRequest {
                    image,
                    collect_dir: &collect_dir,
                    output_dir: &output_dir,
                    matched_expected_images: &matched_expected_images,
                    archive_path,
                    reuse_details: &mut reuse_details,
                    command: ImageCommandContext {
                        execution: &execution,
                        policy,
                        log_sink: log_sink.clone(),
                        cancel_check: cancel_check.clone(),
                    },
                })?);
            }
            state_details.push(("backend_mode".to_string(), "buildroot".to_string()));
            state_details.push((
                "buildroot_dir".to_string(),
                buildroot_dir.display().to_string(),
            ));
            state_details.push((
                "buildroot_output_digest".to_string(),
                buildroot_state_digest(image, &output_dir),
            ));
            state_details.push((
                "matched_expected_images".to_string(),
                matched_expected_images.join(","),
            ));
            messages.push(format!(
                "buildroot image built using backend '{}' into '{}'",
                buildroot_dir.display(),
                output_dir.display()
            ));
        } else if buildroot_allow_fallback(image) {
            let fallback_rootfs_dir = collect_dir.join("rootfs");
            let matched_expected_images =
                materialize_fallback_rootfs(spec, image, &fallback_rootfs_dir)?;
            if let Some(archive_path) = &archive_path {
                messages.extend(archive_directory(
                    &fallback_rootfs_dir,
                    archive_path,
                    "buildroot fallback archive",
                    &execution,
                    policy,
                    log_sink.clone(),
                    cancel_check.clone(),
                )?);
            }
            state_details.push(("backend_mode".to_string(), "fallback".to_string()));
            state_details.push((
                "buildroot_output_digest".to_string(),
                dir_digest(&fallback_rootfs_dir),
            ));
            state_details.push((
                "matched_expected_images".to_string(),
                matched_expected_images.join(","),
            ));
            messages.push(format!(
                "buildroot backend unavailable; assembled fallback rootfs for defconfig '{}'",
                defconfig
            ));
        } else {
            return Err(ImageProviderError::new(
                ImageProviderErrorKind::OutputMissing,
                "buildroot backend unavailable and image.buildroot.allow_fallback is false",
            ));
        }

        let result = ImageExecutionResult {
            provider_id: self.id().into(),
            collect_dir: Some(collect_dir),
            archive_path,
            emit_report: output.emit_report,
            reused: !reuse_details.is_empty(),
            reuse_details,
            messages,
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

    fn execute_image_operation(
        &self,
        request: gaia_image_providers::ImageOperationExecution<'_>,
    ) -> Result<ImageExecutionResult, ImageProviderError> {
        match request.operation {
            ImageProviderOperation::Prepare => {
                let (defconfig, defconfig_path) = match &request.image.definition {
                    ImageDefinition::Buildroot(buildroot) => (
                        buildroot
                            .defconfig
                            .clone()
                            .unwrap_or_else(|| "default".into()),
                        buildroot.defconfig_path.clone(),
                    ),
                    _ => ("default".into(), None),
                };
                let collect_dir = request
                    .output
                    .collect_dir
                    .as_ref()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("out/images/buildroot"));
                let execution = execution_context(request.spec);
                let mut state_details = vec![("defconfig".to_string(), defconfig)];
                if let Some(path) = &defconfig_path {
                    state_details.push(("defconfig_path".to_string(), path.clone()));
                }
                let buildroot_dir = resolve_buildroot_dir(request.spec, request.image).ok_or_else(|| {
                    ImageProviderError::new(
                        ImageProviderErrorKind::OutputMissing,
                        "buildroot backend unavailable and image.buildroot.allow_fallback is false",
                    )
                })?;
                let output_dir = collect_dir.join("buildroot-output");
                let messages = run_buildroot(BuildrootRunRequest {
                    spec: request.spec,
                    image: request.image,
                    buildroot_dir: &buildroot_dir,
                    output_dir: &output_dir,
                    command: ImageCommandContext {
                        execution: &execution,
                        policy: request.policy,
                        log_sink: request.log_sink,
                        cancel_check: request.cancel_check,
                    },
                })?;
                let result = ImageExecutionResult {
                    provider_id: self.id().into(),
                    collect_dir: Some(collect_dir),
                    archive_path: None,
                    emit_report: false,
                    reused: false,
                    reuse_details: Vec::new(),
                    messages,
                    state_details: {
                        let mut details = state_details;
                        details.push(("backend_mode".to_string(), "buildroot-prepare".to_string()));
                        details.push((
                            "buildroot_dir".to_string(),
                            buildroot_dir.display().to_string(),
                        ));
                        details.push((
                            "buildroot_output_digest".to_string(),
                            buildroot_state_digest(request.image, &output_dir),
                        ));
                        details.extend(build_state_details(request.spec));
                        details.extend(build_image_contract_state_details(request.image));
                        details
                    },
                };
                materialize_image_output(&result)?;
                Ok(result)
            }
            ImageProviderOperation::Build => self.execute_image(
                request.spec,
                request.image,
                request.output,
                request.policy,
                request.log_sink,
                request.cancel_check,
            ),
        }
    }
}

mod archive;
mod buildroot;
mod buildroot_external;
mod command;
mod feed;
mod fs_util;
mod squashfs;
#[cfg(test)]
mod tests;

pub(crate) use archive::*;
pub(crate) use buildroot::*;
pub(crate) use buildroot_external::*;
pub(crate) use command::*;
pub(crate) use feed::*;
pub(crate) use fs_util::*;
pub(crate) use squashfs::*;
