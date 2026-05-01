use super::*;

pub(crate) struct MutableRawImageRequest<'a> {
    pub(crate) spec: &'a ResolvedBuildSpec,
    pub(crate) image: &'a ImageSpec,
    pub(crate) source_image: &'a Path,
    pub(crate) collect_dir: &'a Path,
    pub(crate) final_image_path: &'a Path,
    pub(crate) policy: &'a ImageExecutionPolicy,
    pub(crate) log_sink: Option<ProcessLogSink>,
    pub(crate) cancel_check: Option<ProcessCancelCheck>,
}

pub(crate) fn materialize_mutable_raw_image(
    request: MutableRawImageRequest<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    let MutableRawImageRequest {
        spec,
        image,
        source_image,
        collect_dir,
        final_image_path,
        policy,
        log_sink,
        cancel_check,
    } = request;
    let starting_point = starting_point_spec(image)?;
    if image_feed_has_runtime_content(image) && starting_point.image_read_only {
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::PolicyBlocked,
            "starting-point raw image mutation requires image_read_only=false when applying image feed".to_string(),
        ));
    }
    if starting_point.packages.enabled
        && starting_point.packages.execute
        && starting_point.image_read_only
    {
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::PolicyBlocked,
            "starting-point raw image package execution requires image_read_only=false".to_string(),
        ));
    }
    ensure_linux_root("starting-point raw image mutation requires root")?;
    if let Some(parent) = final_image_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to create starting-point raw image output dir '{}': {error}",
                parent.display()
            ))
        })?;
    }
    fs::copy(source_image, final_image_path).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to copy starting-point raw image '{}' to '{}': {error}",
            source_image.display(),
            final_image_path.display()
        ))
    })?;
    let work_root = collect_dir.join("raw-image-work");
    let mount_dir = work_root.join("mount");
    let extract_dir = collect_dir.join("rootfs");
    remove_path_if_exists(&work_root)?;
    remove_path_if_exists(&extract_dir)?;
    fs::create_dir_all(&mount_dir).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to create starting-point image mount dir '{}': {error}",
            mount_dir.display()
        ))
    })?;
    let loop_device = losetup_attach(
        final_image_path,
        policy,
        log_sink.clone(),
        cancel_check.clone(),
    )?;
    let mut raw_runtime =
        RawImageRuntimeGuard::new(loop_device, mount_dir.clone(), policy, log_sink.clone());
    let result = (|| -> Result<Vec<String>, ImageProviderError> {
        let partitions = list_image_partitions(
            raw_runtime.loop_device(),
            policy,
            log_sink.clone(),
            cancel_check.clone(),
        )?;
        let selected = if partitions.is_empty() {
            ImagePartitionInfo {
                path: raw_runtime.loop_device().to_string(),
                fstype: String::new(),
                size_bytes: fs::metadata(final_image_path)
                    .map(|metadata| metadata.len())
                    .unwrap_or(0),
            }
        } else {
            choose_image_partition(&partitions, starting_point.image_partition.as_deref())?
        };
        raw_runtime.mount(
            &selected.path,
            starting_point.image_read_only,
            cancel_check.clone(),
        )?;
        let mut messages = reconcile_packages_in_rootfs(
            &mount_dir,
            &starting_point.packages,
            policy,
            log_sink.clone(),
            cancel_check.clone(),
        )?;
        apply_image_feed_to_rootfs(spec, image, &mount_dir)?;
        if matches!(
            starting_point.output_mode,
            StartingPointOutputModeSpec::CopyRootfs | StartingPointOutputModeSpec::CopyAndArchive
        ) {
            copy_dir(&mount_dir, &extract_dir)?;
        }
        messages.push(format!(
            "starting-point raw image mutated through partition '{}'",
            selected.path
        ));
        Ok(messages)
    })();
    combine_primary_and_cleanup(result, raw_runtime.cleanup())
}

pub(crate) fn image_feed_has_runtime_content(image: &ImageSpec) -> bool {
    !image.feed.install_entries.is_empty()
        || !image.feed.stage_files.is_empty()
        || !image.feed.stage_env_sets.is_empty()
        || !image.feed.stage_services.is_empty()
}

pub(crate) struct RawImageRuntimeGuard<'a> {
    loop_device: String,
    mount_dir: PathBuf,
    mounted: bool,
    pub(crate) policy: &'a ImageExecutionPolicy,
    pub(crate) log_sink: Option<ProcessLogSink>,
    cleaned: bool,
}

impl<'a> RawImageRuntimeGuard<'a> {
    fn new(
        loop_device: String,
        mount_dir: PathBuf,
        policy: &'a ImageExecutionPolicy,
        log_sink: Option<ProcessLogSink>,
    ) -> Self {
        Self {
            loop_device,
            mount_dir,
            mounted: false,
            policy,
            log_sink,
            cleaned: false,
        }
    }

    fn loop_device(&self) -> &str {
        &self.loop_device
    }

    fn mount(
        &mut self,
        partition_path: &str,
        read_only: bool,
        cancel_check: Option<ProcessCancelCheck>,
    ) -> Result<(), ImageProviderError> {
        let mut mount_cmd = Command::new("mount");
        if read_only {
            mount_cmd.arg("-o").arg("ro");
        }
        mount_cmd.arg(partition_path).arg(&self.mount_dir);
        command_status(
            &mut mount_cmd,
            "starting-point raw image mount",
            ImageProviderErrorKind::BackendCommand,
            self.policy,
            self.log_sink.clone(),
            cancel_check,
        )?;
        self.mounted = true;
        Ok(())
    }

    fn cleanup(mut self) -> Result<(), ImageProviderError> {
        let result = self.cleanup_inner();
        self.cleaned = true;
        result
    }

    fn cleanup_inner(&mut self) -> Result<(), ImageProviderError> {
        let mut first_error: Option<ImageProviderError> = None;
        if self.mounted {
            let mut umount = Command::new("umount");
            umount.arg(&self.mount_dir);
            if let Err(error) = cleanup_command_status(
                &mut umount,
                "starting-point raw image unmount",
                ImageProviderErrorKind::RuntimeState,
                self.policy,
                self.log_sink.clone(),
                None,
            ) {
                first_error = Some(error);
            } else {
                self.mounted = false;
            }
        }
        let mut detach = Command::new("losetup");
        detach.arg("-d").arg(&self.loop_device);
        if let Err(error) = cleanup_command_status(
            &mut detach,
            "starting-point raw image detach",
            ImageProviderErrorKind::RuntimeState,
            self.policy,
            self.log_sink.clone(),
            None,
        ) && first_error.is_none()
        {
            first_error = Some(error);
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }
}

impl Drop for RawImageRuntimeGuard<'_> {
    fn drop(&mut self) {
        if !self.cleaned {
            let _ = self.cleanup_inner();
        }
    }
}

pub(crate) fn combine_primary_and_cleanup<T>(
    primary: Result<T, ImageProviderError>,
    cleanup: Result<(), ImageProviderError>,
) -> Result<T, ImageProviderError> {
    match (primary, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(cleanup_error)) => Err(cleanup_error),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup_error)) => Err(ImageProviderError::new(
            error.kind,
            format!(
                "{}; additionally cleanup failed: {}",
                error.message, cleanup_error.message
            ),
        )),
    }
}

pub(crate) fn looks_like_raw_image(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            let lowered = name.to_ascii_lowercase();
            lowered.ends_with(".img") || lowered.ends_with(".raw")
        })
        .unwrap_or(false)
}
