use super::*;

pub(crate) fn resolve_starting_point_rootfs(
    spec: &ResolvedBuildSpec,
    starting_point: &gaia_spec::StartingPointImageSpec,
) -> Result<PathBuf, ImageProviderError> {
    if let Some(source_id) = &starting_point.source {
        let mut root = starting_point_source_dir(spec, source_id);
        if let Some(source_path) = &starting_point.source_path
            && !source_path.trim().is_empty()
        {
            root = root.join(source_path);
        }
        return Ok(root);
    }
    Ok(PathBuf::from(&starting_point.rootfs_path))
}

pub(crate) fn starting_point_source_dir(spec: &ResolvedBuildSpec, source_id: &SourceId) -> PathBuf {
    Path::new(&spec.workspace.root_dir)
        .join(&spec.workspace.build_dir)
        .join("sources")
        .join(source_id.as_str())
}

pub(crate) fn validate_rootfs(
    rootfs: &Path,
    mode: StartingPointRootfsValidationModeSpec,
) -> Result<(), ImageProviderError> {
    match mode {
        StartingPointRootfsValidationModeSpec::AllowMissing => Ok(()),
        StartingPointRootfsValidationModeSpec::RequireExists => {
            if rootfs.exists() {
                Ok(())
            } else {
                Err(ImageProviderError::new(
                    ImageProviderErrorKind::OutputMissing,
                    format!(
                        "starting-point rootfs '{}' does not exist",
                        rootfs.display()
                    ),
                ))
            }
        }
        StartingPointRootfsValidationModeSpec::RequireDirectory => {
            if rootfs.is_dir() {
                Ok(())
            } else {
                Err(ImageProviderError::new(
                    ImageProviderErrorKind::OutputMissing,
                    format!(
                        "starting-point rootfs '{}' must be an existing directory",
                        rootfs.display()
                    ),
                ))
            }
        }
        StartingPointRootfsValidationModeSpec::RequireFile => {
            if rootfs.is_file() {
                Ok(())
            } else {
                Err(ImageProviderError::new(
                    ImageProviderErrorKind::OutputMissing,
                    format!(
                        "starting-point rootfs '{}' must be an existing file",
                        rootfs.display()
                    ),
                ))
            }
        }
    }
}

pub(crate) fn materialize_rootfs(
    rootfs: &Path,
    collect_dir: &Path,
) -> Result<(), ImageProviderError> {
    fs::create_dir_all(collect_dir).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to create starting-point collect dir '{}': {error}",
            collect_dir.display()
        ))
    })?;

    let dest = collect_dir.join("rootfs");
    if dest.exists() {
        if dest.is_dir() {
            fs::remove_dir_all(&dest).map_err(|error| {
                ImageProviderError::backend_command(format!(
                    "failed to clear existing starting-point rootfs dir '{}': {error}",
                    dest.display()
                ))
            })?;
        } else {
            fs::remove_file(&dest).map_err(|error| {
                ImageProviderError::backend_command(format!(
                    "failed to clear existing starting-point rootfs file '{}': {error}",
                    dest.display()
                ))
            })?;
        }
    }

    if rootfs.is_dir() {
        copy_dir(rootfs, &dest)
    } else {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                ImageProviderError::backend_command(format!(
                    "failed to create starting-point rootfs parent '{}': {error}",
                    parent.display()
                ))
            })?;
        }
        fs::copy(rootfs, &dest).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to copy starting-point rootfs '{}' to '{}': {error}",
                rootfs.display(),
                dest.display()
            ))
        })?;
        Ok(())
    }
}

pub(crate) struct MutableRootfsRequest<'a> {
    pub(crate) spec: &'a ResolvedBuildSpec,
    pub(crate) image: &'a ImageSpec,
    pub(crate) rootfs: &'a Path,
    pub(crate) collect_dir: &'a Path,
    pub(crate) execution: &'a ImageExecutionContext,
    pub(crate) policy: &'a ImageExecutionPolicy,
    pub(crate) log_sink: Option<ProcessLogSink>,
    pub(crate) cancel_check: Option<ProcessCancelCheck>,
    pub(crate) messages: &'a mut Vec<String>,
}

pub(crate) fn materialize_mutable_rootfs(
    request: MutableRootfsRequest<'_>,
) -> Result<PathBuf, ImageProviderError> {
    let MutableRootfsRequest {
        spec,
        image,
        rootfs,
        collect_dir,
        execution,
        policy,
        log_sink,
        cancel_check,
        messages,
    } = request;
    fs::create_dir_all(collect_dir).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to create starting-point collect dir '{}': {error}",
            collect_dir.display()
        ))
    })?;
    let dest = collect_dir.join("rootfs");
    if dest.exists() {
        fs::remove_dir_all(&dest).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to clear existing starting-point mutable rootfs '{}': {error}",
                dest.display()
            ))
        })?;
    }
    if rootfs.is_dir() {
        copy_dir(rootfs, &dest)?;
    } else if looks_like_tar_archive(rootfs) {
        extract_tar_archive(
            rootfs,
            &dest,
            execution,
            policy,
            log_sink.clone(),
            cancel_check.clone(),
        )?;
    } else if image_feed_has_runtime_content(image) {
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::PolicyBlocked,
            format!(
                "starting-point rootfs '{}' is an opaque file and cannot accept install/stage overlay; use a directory or tar archive rootfs instead",
                rootfs.display()
            ),
        ));
    } else {
        materialize_rootfs(rootfs, collect_dir)?;
        return Ok(collect_dir.join("rootfs"));
    }
    let package_messages = reconcile_packages_in_rootfs(
        &dest,
        starting_point_packages(image),
        policy,
        log_sink,
        cancel_check,
    )?;
    messages.extend(package_messages);
    apply_image_feed_to_rootfs(spec, image, &dest)?;
    Ok(dest)
}
