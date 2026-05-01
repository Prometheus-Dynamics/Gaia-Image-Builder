use super::*;

#[derive(Debug, Default)]
pub(crate) struct ChrootRuntimeState {
    mounted_targets: Vec<PathBuf>,
    resolv_conf_backup: Option<PathBuf>,
    resolv_conf_created: bool,
}

pub(crate) struct ChrootRuntimeGuard<'a> {
    rootfs_dir: &'a Path,
    state: ChrootRuntimeState,
    policy: &'a ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cleaned: bool,
}

impl<'a> ChrootRuntimeGuard<'a> {
    fn new(
        rootfs_dir: &'a Path,
        policy: &'a ImageExecutionPolicy,
        log_sink: Option<ProcessLogSink>,
    ) -> Self {
        Self {
            rootfs_dir,
            state: ChrootRuntimeState::default(),
            policy,
            log_sink,
            cleaned: false,
        }
    }

    pub(crate) fn cleanup(mut self) -> Result<(), ImageProviderError> {
        let result = self.cleanup_inner();
        self.cleaned = true;
        result
    }

    fn cleanup_inner(&mut self) -> Result<(), ImageProviderError> {
        cleanup_chroot_runtime(
            self.rootfs_dir,
            &mut self.state,
            self.policy,
            self.log_sink.clone(),
        )
    }
}

impl Drop for ChrootRuntimeGuard<'_> {
    fn drop(&mut self) {
        if !self.cleaned {
            let _ = self.cleanup_inner();
        }
    }
}

pub(crate) fn prepare_chroot_runtime<'a>(
    rootfs_dir: &'a Path,
    policy: &'a ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<ChrootRuntimeGuard<'a>, ImageProviderError> {
    let mut guard = ChrootRuntimeGuard::new(rootfs_dir, policy, log_sink.clone());
    for rel in ["dev", "proc", "sys"] {
        fs::create_dir_all(rootfs_dir.join(rel)).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to create chroot runtime dir '{}': {error}",
                rootfs_dir.join(rel).display()
            ))
        })?;
    }
    mount_runtime_target(
        Some(Path::new("/dev")),
        &rootfs_dir.join("dev"),
        Some("--bind"),
        &mut guard.state,
        policy,
        log_sink.clone(),
        cancel_check.clone(),
    )?;
    mount_runtime_target(
        None,
        &rootfs_dir.join("proc"),
        Some("-tproc"),
        &mut guard.state,
        policy,
        log_sink.clone(),
        cancel_check.clone(),
    )?;
    mount_runtime_target(
        None,
        &rootfs_dir.join("sys"),
        Some("-tsysfs"),
        &mut guard.state,
        policy,
        log_sink,
        cancel_check,
    )?;
    prepare_resolv_conf(rootfs_dir, &mut guard.state)?;
    Ok(guard)
}

pub(crate) fn mount_runtime_target(
    source: Option<&Path>,
    target: &Path,
    mode: Option<&str>,
    state: &mut ChrootRuntimeState,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(), ImageProviderError> {
    let mut cmd = Command::new("mount");
    match mode {
        Some("--bind") => {
            let Some(source) = source else {
                return Err(ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "starting-point chroot bind mount '{}' is missing a source path",
                        target.display()
                    ),
                ));
            };
            cmd.arg("--bind");
            cmd.arg(source);
            cmd.arg(target);
        }
        Some("-tproc") => {
            cmd.arg("-t").arg("proc").arg("proc").arg(target);
        }
        Some("-tsysfs") => {
            cmd.arg("-t").arg("sysfs").arg("sysfs").arg(target);
        }
        _ => {
            return Err(ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "starting-point chroot mount '{}' has unsupported mount mode {:?}",
                    target.display(),
                    mode
                ),
            ));
        }
    }
    command_status(
        &mut cmd,
        "starting-point chroot runtime mount",
        ImageProviderErrorKind::RuntimeState,
        policy,
        log_sink,
        cancel_check,
    )?;
    state.mounted_targets.push(target.to_path_buf());
    Ok(())
}

pub(crate) fn prepare_resolv_conf(
    rootfs_dir: &Path,
    state: &mut ChrootRuntimeState,
) -> Result<(), ImageProviderError> {
    let host_resolv = Path::new("/etc/resolv.conf");
    if !host_resolv.is_file() {
        return Ok(());
    }
    let target = rootfs_dir.join("etc/resolv.conf");
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to create resolv.conf parent '{}': {error}",
                parent.display()
            ))
        })?;
    }
    if target.exists() {
        let backup = rootfs_dir.join("etc/.gaia-resolv.conf.bak");
        fs::copy(&target, &backup).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to backup resolv.conf '{}': {error}",
                target.display()
            ))
        })?;
        state.resolv_conf_backup = Some(backup);
    } else {
        state.resolv_conf_created = true;
    }
    fs::copy(host_resolv, &target).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to copy host resolv.conf into '{}': {error}",
            target.display()
        ))
    })?;
    Ok(())
}

pub(crate) fn cleanup_chroot_runtime(
    rootfs_dir: &Path,
    state: &mut ChrootRuntimeState,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
) -> Result<(), ImageProviderError> {
    let mut first_error: Option<ImageProviderError> = None;
    while let Some(target) = state.mounted_targets.pop() {
        let mut cmd = Command::new("umount");
        cmd.arg(&target);
        if let Err(error) = cleanup_command_status(
            &mut cmd,
            "starting-point chroot runtime unmount",
            ImageProviderErrorKind::RuntimeState,
            policy,
            log_sink.clone(),
            None,
        ) && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    let target = rootfs_dir.join("etc/resolv.conf");
    if let Some(backup) = state.resolv_conf_backup.take() {
        if target.exists()
            && let Err(error) = fs::remove_file(&target)
            && first_error.is_none()
        {
            first_error = Some(ImageProviderError::backend_command(format!(
                "failed to remove temporary resolv.conf '{}': {error}",
                target.display()
            )));
        }
        if let Err(error) = fs::rename(&backup, &target)
            && first_error.is_none()
        {
            first_error = Some(ImageProviderError::backend_command(format!(
                "failed to restore resolv.conf backup '{}' to '{}': {error}",
                backup.display(),
                target.display()
            )));
        }
    } else if state.resolv_conf_created
        && target.exists()
        && let Err(error) = fs::remove_file(&target)
        && first_error.is_none()
    {
        first_error = Some(ImageProviderError::backend_command(format!(
            "failed to remove temporary resolv.conf '{}': {error}",
            target.display()
        )));
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    Ok(())
}
