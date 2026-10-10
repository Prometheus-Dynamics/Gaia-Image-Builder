use gaia_image_providers::{
    ImageExecutionPolicy, ImageExecutionResult, ImageOutputContract, ImagePlan, ImagePreview,
    ImageProvider, ImageProviderError, ImageProviderErrorKind, ImageProviderOperation,
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
use std::path::{Component, Path, PathBuf};
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

    fn preview_image(
        &self,
        spec: &ResolvedBuildSpec,
        image: &ImageSpec,
        policy: &ImageExecutionPolicy,
        _operation: ImageProviderOperation,
    ) -> Result<Option<ImagePreview>, ImageProviderError> {
        // Prepare and build plan the same tree, so one preview covers both.
        if !matches!(image.definition, ImageDefinition::Buildroot(_)) {
            return Ok(None);
        }
        preview::preview_buildroot(spec, image, policy).map(Some)
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
            // The tree's real directory: the output dir, or the RAM or work
            // dir it links to (what Buildroot and Docker builds are given).
            let mut tree_dir = output_dir.clone();
            let mut ram_work = None;
            let command = ImageCommandContext {
                execution: &execution,
                policy,
                log_sink: log_sink.clone(),
                cancel_check: cancel_check.clone(),
            };
            if policy.shared_output {
                let clock = gaia_process::ActiveClock::start();
                let shared =
                    shared_buildroot_output(spec, image, &buildroot_dir, policy, &execution)?;
                messages.push(phase_step_message("shared output key", &clock, &[]));
                messages.extend(timed_phase("shared build", || {
                    build_with_shared_output(SharedBuildRequest {
                        spec,
                        image,
                        buildroot_dir: &buildroot_dir,
                        output_dir: &output_dir,
                        shared: &shared,
                        command,
                    })
                })?);
                state_details.push((
                    "buildroot_shared_output_dir".to_string(),
                    shared.dir.display().to_string(),
                ));
                state_details.push(("buildroot_shared_key".to_string(), shared.key.clone()));
            } else {
                let tree = place_private_tree(
                    &output_dir,
                    &buildroot_dir,
                    policy,
                    cancel_check.clone(),
                    log_sink.clone(),
                )?;
                messages.extend(tree.messages.iter().cloned());
                let command = ImageCommandContext {
                    cancel_check: tree.cancel_check.clone(),
                    ..command
                };
                messages.extend(build_with_private_output(
                    spec,
                    image,
                    &tree.source,
                    &tree.work.dir,
                    command,
                )?);
                drop(tree.watchdog);
                tree_dir = tree.work.dir.clone();
                ram_work = Some(tree.work);
            }
            // Buildroot ignores a failed `modules_install`; catch the gap
            // before the images are collected and published.
            messages.extend(timed_phase("kernel modules check", || {
                check_kernel_modules_installed(&tree_dir, policy.kernel_modules_check)
            })?);
            let clock = gaia_process::ActiveClock::start();
            let collected = collect_expected_images_hashed(image, &tree_dir, &collect_dir)?;
            messages.push(phase_step_message("collect expected images", &clock, &[]));
            let matched_expected_images = collected.matched;
            let collected_digests = collected.digests;
            if let Some(archive_path) = &archive_path
                && should_archive_buildroot_output(image, archive_path)
            {
                messages.extend(timed_phase("buildroot archive checks", || {
                    archive_buildroot_output(BuildrootArchiveRequest {
                        image,
                        collect_dir: &collect_dir,
                        output_dir: &tree_dir,
                        matched_expected_images: &matched_expected_images,
                        archive_path,
                        reuse_details: &mut reuse_details,
                        command: ImageCommandContext {
                            execution: &execution,
                            policy,
                            log_sink: log_sink.clone(),
                            cancel_check: cancel_check.clone(),
                        },
                    })
                })?);
            } else if let Some(archive_path) = &archive_path {
                messages.push(format!(
                    "deferred raw image archive '{}' to typed image assembly",
                    archive_path.display()
                ));
            }
            // The images are collected: a RAM tree can be recorded or dropped.
            if let Some(work) = &ram_work {
                messages.extend(finish_ram_tree(
                    &output_dir,
                    work,
                    policy.work_dir.keep_ram_tree,
                ));
            }
            let clock = gaia_process::ActiveClock::start();
            let output_digest = buildroot_state_digest_with(image, &output_dir, &collected_digests);
            messages.push(phase_step_message("buildroot state digest", &clock, &[]));
            state_details.push(("backend_mode".to_string(), "buildroot".to_string()));
            state_details.push((
                "buildroot_dir".to_string(),
                buildroot_dir.display().to_string(),
            ));
            state_details.push(("buildroot_output_digest".to_string(), output_digest));
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

        let mut result = ImageExecutionResult {
            provider_id: self.id().into(),
            collect_dir: Some(collect_dir),
            archive_path,
            emit_report: output.emit_report,
            reused: !reuse_details.is_empty(),
            reuse_details,
            warnings: override_check_warnings(&messages),
            notes: summary_notes(&messages),
            messages,
            state_details: {
                let mut details = state_details;
                details.extend(build_state_details(spec));
                details.extend(build_image_contract_state_details(image));
                details
            },
        };
        let steps = materialize_image_output(&result)?;
        result.messages.extend(steps);
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
                let mut messages = if request.policy.shared_output {
                    let clock = gaia_process::ActiveClock::start();
                    let shared = shared_buildroot_output(
                        request.spec,
                        request.image,
                        &buildroot_dir,
                        request.policy,
                        &execution,
                    )?;
                    let mut messages = vec![phase_step_message("shared output key", &clock, &[])];
                    messages.extend(timed_phase("shared prepare", || {
                        prepare_with_shared_output(SharedBuildRequest {
                            spec: request.spec,
                            image: request.image,
                            buildroot_dir: &buildroot_dir,
                            output_dir: &output_dir,
                            shared: &shared,
                            command,
                        })
                    })?);
                    messages
                } else {
                    let tree = place_private_tree(
                        &output_dir,
                        &buildroot_dir,
                        request.policy,
                        command.cancel_check.clone(),
                        command.log_sink.clone(),
                    )?;
                    let command = ImageCommandContext {
                        cancel_check: tree.cancel_check.clone(),
                        ..command
                    };
                    let mut messages = tree.messages;
                    messages.extend(run_buildroot(BuildrootRunRequest {
                        spec: request.spec,
                        image: request.image,
                        buildroot_dir: &tree.source,
                        output_dir: &tree.work.dir,
                        command,
                    })?);
                    drop(tree.watchdog);
                    messages
                };
                let clock = gaia_process::ActiveClock::start();
                let output_digest = buildroot_state_digest(request.image, &output_dir);
                messages.push(phase_step_message("buildroot state digest", &clock, &[]));
                let mut result = ImageExecutionResult {
                    provider_id: self.id().into(),
                    collect_dir: Some(collect_dir),
                    archive_path: None,
                    emit_report: false,
                    reused: false,
                    reuse_details: Vec::new(),
                    warnings: override_check_warnings(&messages),
                    notes: summary_notes(&messages),
                    messages,
                    state_details: {
                        let mut details = state_details;
                        details.push(("backend_mode".to_string(), "buildroot-prepare".to_string()));
                        details.push((
                            "buildroot_dir".to_string(),
                            buildroot_dir.display().to_string(),
                        ));
                        details.push(("buildroot_output_digest".to_string(), output_digest));
                        details.extend(build_state_details(request.spec));
                        details.extend(build_image_contract_state_details(request.image));
                        details
                    },
                };
                let steps = materialize_image_output(&result)?;
                result.messages.extend(steps);
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

/// A tree a build works in outside the shared views (see [`place_work_dir`]):
/// placed, watched while it is in RAM, and given the Buildroot source it
/// reads (a mirror of it for a RAM tree).
struct PrivateTree {
    work: WorkDir,
    source: PathBuf,
    /// Stops the tree's commands when memory runs low (RAM trees only).
    watchdog: Option<RamWatchdog>,
    /// The cancel check of the tree's commands: the caller's, or the
    /// watchdog's, which also reports low memory.
    cancel_check: Option<ProcessCancelCheck>,
    messages: Vec<String>,
}

fn place_private_tree(
    output_dir: &Path,
    buildroot_dir: &Path,
    policy: &ImageExecutionPolicy,
    cancel_check: Option<ProcessCancelCheck>,
    log_sink: Option<ProcessLogSink>,
) -> Result<PrivateTree, ImageProviderError> {
    let clock = gaia_process::ActiveClock::start();
    let mut messages = leave_shared_view(output_dir)?;
    let work = place_work_dir(output_dir, policy)?;
    messages.extend(work.messages.iter().cloned());
    messages.push(phase_step_message("work dir placement", &clock, &[]));
    let clock = gaia_process::ActiveClock::start();
    let watchdog = work
        .ram
        .then(|| RamWatchdog::start(&work.dir, cancel_check.clone(), log_sink));
    if work.ram {
        messages.push(phase_step_message("ram watchdog start", &clock, &[]));
    }
    let cancel_check = watchdog
        .as_ref()
        .map(|(_, check)| check.clone())
        .or(cancel_check);
    let clock = gaia_process::ActiveClock::start();
    let (source, note) = buildroot_source_for(buildroot_dir, &work);
    messages.extend(note);
    messages.push(phase_step_message("buildroot source mirror", &clock, &[]));
    Ok(PrivateTree {
        work,
        source,
        watchdog: watchdog.map(|(watchdog, _)| watchdog),
        cancel_check,
        messages,
    })
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
    let clock = gaia_process::ActiveClock::start();
    let staged_feed = stage_image_feed_for_make(spec, image, output_dir)?;
    let staging_step = phase_step_message("image feed staging", &clock, &[]);
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
            finalize_only: false,
        },
    )?;
    messages.push(staging_step);
    let Some(staged_feed) = staged_feed else {
        let clock = gaia_process::ActiveClock::start();
        remove_path_if_exists(&image_feed_signature_path(output_dir))?;
        if image_feed_managed_paths_path(output_dir).is_file() && target_dir.is_dir() {
            invalidate_finalized(output_dir);
            prune_stale_image_feed_outputs(spec, image, &target_dir, output_dir)?;
        }
        remove_path_if_exists(&image_feed_managed_paths_path(output_dir))?;
        messages.push(phase_step_message("image feed cleanup", &clock, &[]));
        return Ok(messages);
    };
    if staged_feed.applied() {
        messages.push("applied image feed through a Buildroot post-build script".into());
    } else {
        // The feed is applied to target/ below: no longer the finalized tree.
        invalidate_finalized(output_dir);
        tracing::warn!(
            output_dir = %output_dir.display(),
            "Buildroot did not run the image feed post-build script; refreshing images after make"
        );
        messages.push(
            "Buildroot did not run the image feed post-build script; applied the feed after make"
                .into(),
        );
        let clock = gaia_process::ActiveClock::start();
        apply_image_feed_to_rootfs(spec, image, &target_dir)?;
        let refreshed = refresh_buildroot_images_after_feed_overlay(
            spec,
            image,
            buildroot_dir,
            output_dir,
            command.execution,
            command.policy,
            command.log_sink.clone(),
            command.cancel_check.clone(),
        )?;
        messages.push(phase_step_message("image feed apply", &clock, &refreshed));
        messages.extend(refreshed);
    }
    let clock = gaia_process::ActiveClock::start();
    refresh_expected_tar_images(image, &target_dir, output_dir, command.execution)?;
    messages.push(phase_step_message("expected tar images", &clock, &[]));
    let clock = gaia_process::ActiveClock::start();
    write_image_feed_managed_paths(output_dir, spec, image)?;
    write_image_feed_signature(output_dir, &staged_feed.signature)?;
    remove_path_if_exists(&staged_image_feed_dir(output_dir))?;
    messages.push(phase_step_message("image feed records", &clock, &[]));
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
        .is_some_and(|name| gaia_spec::raw_disk_archive(name).is_some());
    !(image
        .assembly
        .as_ref()
        .is_some_and(|assembly| !assembly.disks.is_empty())
        && raw_disk_name)
}

mod archive;
mod build_times;
mod buildroot;
mod buildroot_caches;
mod buildroot_config;
mod buildroot_external;
mod buildroot_patches;
mod clean_decision;
mod clean_plan;
mod command;
mod config_inputs;
mod configure;
mod feed;
mod feed_make;
mod finalize_state;
mod fs_util;
mod host_tools;
mod interrupted_make;
mod kernel_modules;
mod make_progress;
mod override_check;
mod package_cache;
mod package_cache_files;
mod package_graph;
mod package_keys;
mod preview;
mod ram_tree;
mod rebuild_inputs;
mod rootfs_inputs;
mod shared;
mod source_date;
mod squashfs;
mod symbol_use;
#[cfg(test)]
mod tests;
mod work_dir_decision;

/// Moves the run messages meant for the run summary into
/// [`ImageExecutionResult::notes`].
fn summary_notes(messages: &[String]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| message.strip_prefix(SUMMARY_NOTE_PREFIX))
        .map(str::to_string)
        .collect()
}

pub(crate) use archive::*;
pub(crate) use build_times::*;
pub(crate) use buildroot::*;
pub(crate) use buildroot_caches::*;
pub(crate) use buildroot_config::*;
pub(crate) use buildroot_external::*;
pub(crate) use buildroot_patches::*;
pub(crate) use clean_decision::*;
pub(crate) use clean_plan::*;
pub(crate) use command::*;
pub(crate) use config_inputs::*;
pub(crate) use configure::*;
pub(crate) use feed::*;
pub(crate) use feed_make::*;
pub(crate) use finalize_state::*;
pub(crate) use fs_util::*;
pub(crate) use host_tools::*;
pub(crate) use interrupted_make::*;
pub(crate) use kernel_modules::*;
pub(crate) use make_progress::*;
pub(crate) use override_check::*;
pub(crate) use package_cache::*;
pub(crate) use package_cache_files::*;
pub(crate) use package_graph::*;
pub(crate) use package_keys::*;
pub(crate) use ram_tree::*;
pub(crate) use rebuild_inputs::*;
pub(crate) use rootfs_inputs::*;
pub(crate) use shared::*;
pub(crate) use source_date::*;
pub(crate) use squashfs::*;
pub(crate) use symbol_use::*;
pub(crate) use work_dir_decision::*;
