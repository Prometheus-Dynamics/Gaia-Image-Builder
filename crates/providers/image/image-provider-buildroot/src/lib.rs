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
            .map(|dir| resolve_collect_dir(spec, dir))
            .unwrap_or_else(|| default_collect_dir(spec));
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
            let output_dir = buildroot_output_dir(spec);
            let command = ImageCommandContext {
                execution: &execution,
                policy,
                log_sink: log_sink.clone(),
                cancel_check: cancel_check.clone(),
            };
            if policy.shared_output {
                let shared =
                    shared_buildroot_output(spec, image, &buildroot_dir, policy, &execution)?;
                messages.extend(build_with_shared_output(SharedBuildRequest {
                    spec,
                    image,
                    buildroot_dir: &buildroot_dir,
                    output_dir: &output_dir,
                    shared: &shared,
                    command,
                })?);
                state_details.push((
                    "buildroot_shared_output_dir".to_string(),
                    shared.dir.display().to_string(),
                ));
                state_details.push(("buildroot_shared_key".to_string(), shared.key.clone()));
            } else {
                messages.extend(leave_shared_view(&output_dir)?);
                messages.extend(build_with_private_output(
                    spec,
                    image,
                    &buildroot_dir,
                    &output_dir,
                    command,
                )?);
            }
            let matched_expected_images =
                collect_expected_images(image, &output_dir, &collect_dir)?;
            if let Some(archive_path) = &archive_path
                && should_archive_buildroot_output(image, archive_path)
            {
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
            } else if let Some(archive_path) = &archive_path {
                messages.push(format!(
                    "deferred raw image archive '{}' to typed image assembly",
                    archive_path.display()
                ));
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
                "buildroot_output_dir".to_string(),
                output_dir.display().to_string(),
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
            warnings: override_check_warnings(&messages),
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
                    .map(|dir| resolve_collect_dir(request.spec, dir))
                    .unwrap_or_else(|| default_collect_dir(request.spec));
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
                let output_dir = buildroot_output_dir(request.spec);
                let command = ImageCommandContext {
                    execution: &execution,
                    policy: request.policy,
                    log_sink: request.log_sink,
                    cancel_check: request.cancel_check,
                };
                let messages = if request.policy.shared_output {
                    let shared = shared_buildroot_output(
                        request.spec,
                        request.image,
                        &buildroot_dir,
                        request.policy,
                        &execution,
                    )?;
                    prepare_with_shared_output(SharedBuildRequest {
                        spec: request.spec,
                        image: request.image,
                        buildroot_dir: &buildroot_dir,
                        output_dir: &output_dir,
                        shared: &shared,
                        command,
                    })?
                } else {
                    let mut messages = leave_shared_view(&output_dir)?;
                    messages.extend(run_buildroot(BuildrootRunRequest {
                        spec: request.spec,
                        image: request.image,
                        buildroot_dir: &buildroot_dir,
                        output_dir: &output_dir,
                        command,
                    })?);
                    messages
                };
                let result = ImageExecutionResult {
                    provider_id: self.id().into(),
                    collect_dir: Some(collect_dir),
                    archive_path: None,
                    emit_report: false,
                    reused: false,
                    reuse_details: Vec::new(),
                    warnings: override_check_warnings(&messages),
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

/// Private output tree: one `make` with the image feed delivered by a
/// post-build script, so every image is packed once with the feed included.
/// If Buildroot did not run the script, the feed is applied to `target/` and
/// the images are refreshed the old way.
fn build_with_private_output(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
    buildroot_dir: &Path,
    output_dir: &Path,
    command: ImageCommandContext<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    let target_dir = output_dir.join("target");
    let staged_feed = stage_image_feed_for_make(spec, image, output_dir)?;
    let mut messages = run_buildroot_with(
        BuildrootRunRequest {
            spec,
            image,
            buildroot_dir,
            output_dir,
            command: command.clone(),
        },
        BuildrootMakeOptions {
            post_build_script: staged_feed.as_ref().map(|feed| feed.script.as_path()),
            shared_tree: false,
        },
    )?;
    let Some(staged_feed) = staged_feed else {
        remove_path_if_exists(&image_feed_signature_path(output_dir))?;
        if image_feed_managed_paths_path(output_dir).is_file() && target_dir.is_dir() {
            prune_stale_image_feed_outputs(spec, image, &target_dir, output_dir)?;
        }
        remove_path_if_exists(&image_feed_managed_paths_path(output_dir))?;
        return Ok(messages);
    };
    if staged_feed.applied() {
        messages.push("applied image feed through a Buildroot post-build script".into());
    } else {
        tracing::warn!(
            output_dir = %output_dir.display(),
            "Buildroot did not run the image feed post-build script; refreshing images after make"
        );
        messages.push(
            "Buildroot did not run the image feed post-build script; applied the feed after make"
                .into(),
        );
        apply_image_feed_to_rootfs(spec, image, &target_dir)?;
        messages.extend(refresh_buildroot_images_after_feed_overlay(
            spec,
            image,
            buildroot_dir,
            output_dir,
            command.execution,
            command.policy,
            command.log_sink.clone(),
            command.cancel_check.clone(),
        )?);
    }
    refresh_expected_tar_images(image, &target_dir, output_dir, command.execution)?;
    write_image_feed_managed_paths(output_dir, spec, image)?;
    write_image_feed_signature(output_dir, &staged_feed.signature)?;
    remove_path_if_exists(&staged_image_feed_dir(output_dir))?;
    Ok(messages)
}

/// Relative collect dirs resolve against the workspace root, not the process
/// working directory.
fn resolve_collect_dir(spec: &ResolvedBuildSpec, dir: &str) -> PathBuf {
    let path = PathBuf::from(dir);
    if path.is_absolute() {
        path
    } else {
        PathBuf::from(&spec.workspace.root_dir).join(path)
    }
}

fn default_collect_dir(spec: &ResolvedBuildSpec) -> PathBuf {
    let raw = gaia_spec::default_image_collect_dir(&spec.workspace);
    resolve_workspace_path(spec, &raw)
        .unwrap_or_else(|_| PathBuf::from(&spec.workspace.root_dir).join(raw))
}

fn buildroot_output_dir(spec: &ResolvedBuildSpec) -> PathBuf {
    let build_dir = PathBuf::from(&spec.workspace.build_dir);
    let resolved_build_dir = if build_dir.is_absolute() {
        build_dir
    } else {
        PathBuf::from(&spec.workspace.root_dir).join(build_dir)
    };
    resolved_build_dir.join("image/buildroot-output")
}

/// A raw disk archive name (`.img`, `.raw`, or their `.xz` forms) belongs to
/// typed image assembly when it builds disks. Buildroot would otherwise copy
/// its first expected image, such as a bare rootfs, under the disk's name.
fn should_archive_buildroot_output(image: &ImageSpec, archive_path: &Path) -> bool {
    let raw_disk_name = archive_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            [".img", ".raw", ".img.xz", ".raw.xz"]
                .iter()
                .any(|suffix| name.ends_with(suffix))
        });
    !(image
        .assembly
        .as_ref()
        .is_some_and(|assembly| !assembly.disks.is_empty())
        && raw_disk_name)
}

mod archive;
mod buildroot;
mod buildroot_config;
mod buildroot_external;
mod command;
mod feed;
mod feed_make;
mod fs_util;
mod override_check;
mod rebuild_inputs;
mod shared;
mod squashfs;
#[cfg(test)]
mod tests;

pub(crate) use archive::*;
pub(crate) use buildroot::*;
pub(crate) use buildroot_config::*;
pub(crate) use buildroot_external::*;
pub(crate) use command::*;
pub(crate) use feed::*;
pub(crate) use feed_make::*;
pub(crate) use fs_util::*;
pub(crate) use override_check::*;
pub(crate) use rebuild_inputs::*;
pub(crate) use shared::*;
pub(crate) use squashfs::*;
