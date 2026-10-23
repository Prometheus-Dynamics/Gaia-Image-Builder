//! The private Buildroot tree: its placement (RAM or disk) and the build that runs in it.

use super::*;

/// A tree a build works in outside the shared views (see [`place_work_dir`]):
/// placed, watched while it is in RAM, and given the Buildroot source it
/// reads (a mirror of it for a RAM tree).
pub(super) struct PrivateTree {
    pub(crate) work: WorkDir,
    pub(crate) source: PathBuf,
    /// Stops the tree's commands when memory runs low (RAM trees only).
    pub(crate) watchdog: Option<RamWatchdog>,
    /// The cancel check of the tree's commands: the caller's, or the
    /// watchdog's, which also reports low memory.
    pub(crate) cancel_check: Option<ProcessCancelCheck>,
    pub(crate) messages: Vec<String>,
}

pub(super) fn place_private_tree(
    output_dir: &Path,
    buildroot_dir: &Path,
    policy: &ImageExecutionPolicy,
    canonical: Option<&CanonicalRequest>,
    cancel_check: Option<ProcessCancelCheck>,
    log_sink: Option<ProcessLogSink>,
) -> Result<PrivateTree, ImageProviderError> {
    let clock = gaia_process::ActiveClock::start();
    let mut messages = leave_shared_view(output_dir)?;
    let work = place_work_dir(output_dir, policy, &ram_base(), canonical)?;
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
    let (source, notes) = buildroot_source_for(buildroot_dir, &work, policy.parallel_packages);
    messages.extend(notes);
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
pub(super) fn build_with_private_output(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
    buildroot_dir: &Path,
    output_dir: &Path,
    command: ImageCommandContext<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    let target_dir = output_dir.join("target");
    let rootfs_names = rootfs_exclude_names(image);
    if !rootfs_names.is_empty() && !command.policy.parallel_packages {
        return Err(ImageProviderError::backend_command(
            "image.rootfs.exclude_packages needs per-package directories (policy.providers.buildroot.parallel_packages): without them Buildroot copies the excluded files back into target/ on every finalize",
        ));
    }
    let rootfs_exclude = stage_rootfs_exclude(output_dir, &rootfs_names)?;
    let clock = gaia_process::ActiveClock::start();
    let staged_feed = stage_image_feed_for_make(spec, image, output_dir)?;
    let staging_step = phase_step_message("image feed staging", &clock, &[]);
    let staging_notes = staged_feed
        .as_ref()
        .map(|feed| feed.notes.clone())
        .unwrap_or_default();
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
            rootfs_exclude: rootfs_exclude.as_ref(),
        },
    )?;
    messages.push(staging_step);
    messages.extend(staging_notes);
    let Some(staged_feed) = staged_feed else {
        let clock = gaia_process::ActiveClock::start();
        remove_path_if_exists(&image_feed_signature_path(output_dir))?;
        if image_feed_managed_paths_path(output_dir).is_file() && target_dir.is_dir() {
            invalidate_finalized(output_dir);
            prune_stale_image_feed_outputs(spec, image, &target_dir, output_dir)?;
        }
        remove_path_if_exists(&image_feed_managed_paths_path(output_dir))?;
        messages.extend(clear_stage_ownership(output_dir, &target_dir)?);
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
    record_stage_ownership(output_dir, &staged_feed.ownership, &target_dir)?;
    remove_path_if_exists(&staged_image_feed_dir(output_dir))?;
    messages.push(phase_step_message("image feed records", &clock, &[]));
    Ok(messages)
}
