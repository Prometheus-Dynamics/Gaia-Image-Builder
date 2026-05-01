use super::*;

pub(crate) fn looks_like_tar_archive(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            let lowered = name.to_ascii_lowercase();
            lowered.ends_with(".tar")
                || lowered.ends_with(".tar.gz")
                || lowered.ends_with(".tgz")
                || lowered.ends_with(".tar.xz")
                || lowered.ends_with(".txz")
                || lowered.ends_with(".tar.bz2")
                || lowered.ends_with(".tbz2")
        })
        .unwrap_or(false)
}

pub(crate) fn extract_tar_archive(
    archive_path: &Path,
    dest: &Path,
    execution: &ImageExecutionContext,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(), ImageProviderError> {
    fs::create_dir_all(dest).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to create starting-point extracted rootfs dir '{}': {error}",
            dest.display()
        ))
    })?;
    gaia_process::validate_tar_archive_entries(
        archive_path,
        0,
        Duration::from_secs(policy.timeout_seconds.max(1)),
        "validate starting-point rootfs archive entries",
        log_sink.clone(),
        cancel_check.clone(),
    )
    .map_err(|error| {
        ImageProviderError::new(ImageProviderErrorKind::PolicyBlocked, error.message)
    })?;
    let mut command = Command::new("tar");
    command
        .arg("-xf")
        .arg(archive_path)
        .arg("--no-same-owner")
        .arg("--no-same-permissions")
        .arg("--delay-directory-restore")
        .arg("-C")
        .arg(dest);
    run_command(
        command,
        archive_path,
        execution,
        policy,
        log_sink,
        cancel_check,
    )?;
    flatten_single_rootfs_directory(dest)?;
    Ok(())
}

pub(crate) fn flatten_single_rootfs_directory(dest: &Path) -> Result<(), ImageProviderError> {
    let mut entries = fs::read_dir(dest)
        .map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to read extracted starting-point rootfs '{}': {error}",
                dest.display()
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to read extracted starting-point rootfs entry '{}': {error}",
                dest.display()
            ))
        })?;
    if entries.len() != 1 {
        return Ok(());
    }
    let Some(only) = entries.pop() else {
        return Ok(());
    };
    let only_path = only.path();
    if !only_path.is_dir() {
        return Ok(());
    }
    let temp = dest.with_extension("flatten");
    if temp.exists() {
        fs::remove_dir_all(&temp).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to clear temporary starting-point rootfs dir '{}': {error}",
                temp.display()
            ))
        })?;
    }
    fs::rename(&only_path, &temp).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to move extracted starting-point rootfs '{}' to '{}': {error}",
            only_path.display(),
            temp.display()
        ))
    })?;
    fs::remove_dir_all(dest).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to clear extracted starting-point rootfs dir '{}': {error}",
            dest.display()
        ))
    })?;
    fs::rename(&temp, dest).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to finalize extracted starting-point rootfs '{}' to '{}': {error}",
            temp.display(),
            dest.display()
        ))
    })?;
    Ok(())
}
