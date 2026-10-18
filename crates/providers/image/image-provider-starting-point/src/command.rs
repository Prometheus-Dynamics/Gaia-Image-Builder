use super::*;
use gaia_image_providers::{AttemptFailure, RetryRun, run_with_retries};
use std::collections::HashMap;

pub(crate) fn stderr_or_stdout(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        stderr
    } else {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }
}

pub(crate) fn command_status(
    command: &mut Command,
    label: &str,
    error_kind: ImageProviderErrorKind,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(), ImageProviderError> {
    let output = command_output(command, label, policy, log_sink, cancel_check)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(ImageProviderError::new(
            error_kind,
            format!("{label} failed: {}", stderr_or_stdout(&output)),
        ))
    }
}

pub(crate) fn cleanup_command_status(
    command: &mut Command,
    label: &str,
    error_kind: ImageProviderErrorKind,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(), ImageProviderError> {
    let timeout = Duration::from_secs(
        policy
            .timeout_seconds
            .clamp(1, PRIVILEGED_CLEANUP_TIMEOUT_SECONDS),
    );
    let output = command_output_with_duration(
        command,
        label,
        timeout,
        policy.output_retention,
        log_sink,
        cancel_check,
    )?;
    if output.status.success() {
        Ok(())
    } else {
        Err(ImageProviderError::new(
            error_kind,
            format!("{label} failed: {}", stderr_or_stdout(&output)),
        ))
    }
}

pub(crate) fn command_output(
    command: &mut Command,
    label: &str,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Output, ImageProviderError> {
    command_output_with_duration(
        command,
        label,
        Duration::from_secs(policy.timeout_seconds.max(1)),
        policy.output_retention,
        log_sink,
        cancel_check,
    )
}

pub(crate) fn command_output_with_duration(
    command: &mut Command,
    label: &str,
    timeout: Duration,
    retention: ProcessOutputRetention,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Output, ImageProviderError> {
    run_command_with_timeout_and_retention(
        command,
        timeout,
        label,
        retention,
        log_sink,
        cancel_check,
    )
    .map(|result| result.output)
    .map_err(|error| {
        let kind = match error.kind {
            ProcessRunErrorKind::ToolStart => ImageProviderErrorKind::ToolStart,
            ProcessRunErrorKind::Timeout => ImageProviderErrorKind::Timeout,
            ProcessRunErrorKind::Cancelled => ImageProviderErrorKind::Cancelled,
            ProcessRunErrorKind::RuntimeState => ImageProviderErrorKind::RuntimeState,
        };
        ImageProviderError::new(kind, error.message)
    })
}

pub(crate) fn remove_path_if_exists(path: &Path) -> Result<(), ImageProviderError> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        fs::remove_dir_all(path).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to remove directory '{}': {error}",
                path.display()
            ))
        })
    } else {
        fs::remove_file(path).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to remove file '{}': {error}",
                path.display()
            ))
        })
    }
}

pub(crate) fn create_rootfs_archive(
    rootfs: &Path,
    archive_path: &Path,
    execution: &ImageExecutionContext,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Vec<String>, ImageProviderError> {
    if let Some(parent) = archive_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to create starting-point archive dir '{}': {error}",
                parent.display()
            ))
        })?;
    }

    let mut command = Command::new("tar");
    command.arg("-cf").arg(archive_path);
    if rootfs.is_dir() {
        let parent = rootfs.parent().unwrap_or_else(|| Path::new("/"));
        let name = rootfs
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("rootfs");
        command.arg("-C").arg(parent).arg(name);
    } else {
        let parent = rootfs.parent().unwrap_or_else(|| Path::new("/"));
        let name = rootfs
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("rootfs");
        command.arg("-C").arg(parent).arg(name);
    }
    run_command(
        command,
        archive_path,
        execution,
        policy,
        log_sink,
        cancel_check,
    )
}

pub(crate) fn run_command(
    mut command: Command,
    archive_path: &Path,
    execution: &ImageExecutionContext,
    policy: &ImageExecutionPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Vec<String>, ImageProviderError> {
    let attempts = policy.retry_attempts.max(1);
    let timeout = Duration::from_secs(policy.timeout_seconds.max(1));
    let label = format!("starting-point archive build '{}'", archive_path.display());
    let run = RetryRun {
        domain: "image.starting-point",
        label: &label,
        backend: execution_backend(execution),
        docker_image: execution.docker_image.as_deref(),
        timeout,
        attempts,
        backoff_strategy: policy.retry_backoff_strategy,
        backoff_ms: policy.retry_backoff_ms,
        cancel_check: cancel_check.as_ref(),
    };
    run_with_retries(&run, |attempt| {
        let output = command_output_with_timeout(
            &mut command,
            execution,
            timeout,
            &label,
            log_sink.clone(),
            cancel_check.clone(),
        )?;
        if output.status.success() {
            return Ok(None);
        }
        Ok(Some(AttemptFailure {
            message: format!(
                "starting-point archive build failed for '{}' on attempt {attempt}/{attempts}: {}",
                archive_path.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            retryable: true,
        }))
    })
    .map(|()| Vec::new())
}

fn execution_backend(execution: &ImageExecutionContext) -> &'static str {
    if execution.docker_image.is_some() {
        "docker"
    } else {
        "host"
    }
}

pub(crate) fn command_output_with_timeout(
    command: &mut Command,
    execution: &ImageExecutionContext,
    timeout: Duration,
    label: &str,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<Output, ImageProviderError> {
    let label = label.to_string();
    let wrapped_sink = log_sink.map(|sink| label_process_log_sink(label.clone(), sink));
    let mut exec_command = command_for_execution(command, execution)?;
    run_command_with_timeout(
        &mut exec_command,
        timeout,
        &label,
        wrapped_sink,
        cancel_check,
    )
    .map(|result| result.output)
    .map_err(move |error| {
        let kind = match error.kind {
            ProcessRunErrorKind::ToolStart => ImageProviderErrorKind::ToolStart,
            ProcessRunErrorKind::Timeout => ImageProviderErrorKind::Timeout,
            ProcessRunErrorKind::Cancelled => ImageProviderErrorKind::Cancelled,
            ProcessRunErrorKind::RuntimeState => ImageProviderErrorKind::RuntimeState,
        };
        ImageProviderError::new(kind, error.message)
    })
}

pub(crate) fn execution_context(spec: &ResolvedBuildSpec) -> ImageExecutionContext {
    let workspace_root = PathBuf::from(&spec.workspace.root_dir);
    ImageExecutionContext {
        workspace_root: fs::canonicalize(&workspace_root).unwrap_or(workspace_root),
        docker_image: spec
            .policy
            .execution
            .docker
            .as_ref()
            .map(|docker| docker.image.clone()),
        docker_reuse_container: spec
            .policy
            .execution
            .docker
            .as_ref()
            .is_some_and(|docker| docker.reuse_container),
    }
}

pub(crate) fn command_for_execution(
    command: &Command,
    execution: &ImageExecutionContext,
) -> Result<Command, ImageProviderError> {
    let Some(image) = &execution.docker_image else {
        return Ok(gaia_process::clone_command(command));
    };
    if image.trim().is_empty() {
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::PolicyBlocked,
            "docker execution requires a non-empty image",
        ));
    }
    let spec =
        DockerRunSpec::discovered_mounts(image.clone(), execution.workspace_root.clone(), command);
    let wrapped = if execution.docker_reuse_container {
        docker_reused_exec_command(command, &spec)
    } else {
        docker_run_command(command, &spec)
    };
    wrapped.map_err(|error| {
        ImageProviderError::new(ImageProviderErrorKind::PolicyBlocked, error.to_string())
    })
}

/// Copies a rootfs tree into `dest`, as it is. Symlinks are recreated with
/// their targets and never followed, so a link into the host cannot pull host
/// files into the image. Modes are kept, hardlinks inside the tree stay
/// hardlinks, and device nodes and fifos are recreated when running as root.
pub(crate) fn copy_dir(src: &Path, dest: &Path) -> Result<(), ImageProviderError> {
    copy_tree(src, dest, &mut HashMap::new())
}

/// Files already copied, by (device, inode), so later hardlinks to them can
/// be recreated instead of copied again.
type CopiedFiles = HashMap<(u64, u64), PathBuf>;

fn copy_tree(src: &Path, dest: &Path, copied: &mut CopiedFiles) -> Result<(), ImageProviderError> {
    fs::create_dir_all(dest).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to create starting-point rootfs dir '{}': {error}",
            dest.display()
        ))
    })?;
    for entry in fs::read_dir(src).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to read starting-point rootfs '{}': {error}",
            src.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to read starting-point rootfs entry '{}': {error}",
                src.display()
            ))
        })?;
        let path = entry.path();
        let target = dest.join(entry.file_name());
        let meta = fs::symlink_metadata(&path).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to inspect starting-point rootfs entry '{}': {error}",
                path.display()
            ))
        })?;
        let file_type = meta.file_type();
        if file_type.is_symlink() {
            copy_symlink(&path, &target)?;
        } else if file_type.is_dir() {
            copy_tree(&path, &target, copied)?;
            set_copied_mode(&target, &meta)?;
        } else if file_type.is_file() {
            copy_file(&path, &target, &meta, copied)?;
        } else {
            copy_special_file(&path, &target, &meta)?;
        }
    }
    Ok(())
}

fn copy_symlink(path: &Path, target: &Path) -> Result<(), ImageProviderError> {
    let link = fs::read_link(path).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to read starting-point rootfs symlink '{}': {error}",
            path.display()
        ))
    })?;
    std::os::unix::fs::symlink(&link, target).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to create starting-point rootfs symlink '{}' -> '{}': {error}",
            target.display(),
            link.display()
        ))
    })
}

fn copy_file(
    path: &Path,
    target: &Path,
    meta: &fs::Metadata,
    copied: &mut CopiedFiles,
) -> Result<(), ImageProviderError> {
    use std::os::unix::fs::MetadataExt;

    let identity = (meta.dev(), meta.ino());
    let first = if meta.nlink() > 1 {
        copied.get(&identity).cloned()
    } else {
        None
    };
    if let Some(first) = first {
        return fs::hard_link(&first, target).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to recreate starting-point rootfs hardlink '{}' -> '{}': {error}",
                target.display(),
                first.display()
            ))
        });
    }
    // `fs::copy` keeps the source's permission bits.
    fs::copy(path, target).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to copy starting-point rootfs file '{}' to '{}': {error}",
            path.display(),
            target.display()
        ))
    })?;
    if meta.nlink() > 1 {
        copied.insert(identity, target.to_path_buf());
    }
    Ok(())
}

fn copy_special_file(
    path: &Path,
    target: &Path,
    meta: &fs::Metadata,
) -> Result<(), ImageProviderError> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;

    // SAFETY: `geteuid` only reads the process's credentials.
    if unsafe { libc::geteuid() } != 0 {
        return Err(ImageProviderError::backend_command(format!(
            "cannot copy starting-point rootfs special file '{}' (device, fifo or socket) \
             without root; run gaia as root or remove it from the rootfs",
            path.display()
        )));
    }
    let c_target = std::ffi::CString::new(target.as_os_str().as_bytes()).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "invalid starting-point rootfs path '{}': {error}",
            target.display()
        ))
    })?;
    // SAFETY: `c_target` is a valid NUL-terminated path that outlives the call.
    if unsafe {
        libc::mknod(
            c_target.as_ptr(),
            meta.mode() as libc::mode_t,
            meta.rdev() as libc::dev_t,
        )
    } != 0
    {
        return Err(ImageProviderError::backend_command(format!(
            "failed to recreate starting-point rootfs special file '{}': {}",
            target.display(),
            std::io::Error::last_os_error()
        )));
    }
    // `mknod` applies the umask, so the mode is set again.
    set_copied_mode(target, meta)
}

/// Gives a copied directory or special file the source's permission bits.
fn set_copied_mode(target: &Path, meta: &fs::Metadata) -> Result<(), ImageProviderError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    fs::set_permissions(target, fs::Permissions::from_mode(meta.mode() & 0o7777)).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to set mode on starting-point rootfs entry '{}': {error}",
            target.display()
        ))
    })
}

#[cfg(test)]
mod copy_tests {
    use super::*;
    use gaia_testkit::TempDir;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    /// A host file outside the rootfs that an absolute symlink points at.
    /// Copying must never read it.
    #[test]
    fn copy_dir_keeps_absolute_symlinks_as_links_and_never_copies_host_content() {
        let scratch = TempDir::new("gaia-starting-point-copy");
        let host = scratch.join("host");
        fs::create_dir_all(&host).expect("host dir");
        let host_file = host.join("stub-resolv.conf");
        fs::write(&host_file, "nameserver 10.0.0.1\n").expect("host file");
        let rootfs = scratch.join("rootfs");
        fs::create_dir_all(rootfs.join("etc")).expect("etc");
        symlink(&host_file, rootfs.join("etc/resolv.conf")).expect("absolute symlink");
        let dest = scratch.join("out");

        copy_dir(&rootfs, &dest).expect("copy should succeed");

        let copied = dest.join("etc/resolv.conf");
        assert!(
            fs::symlink_metadata(&copied)
                .expect("copied link")
                .file_type()
                .is_symlink(),
            "the link itself must be copied"
        );
        assert_eq!(fs::read_link(&copied).expect("link target"), host_file);
    }

    #[test]
    fn copy_dir_preserves_relative_symlinks() {
        let scratch = TempDir::new("gaia-starting-point-copy");
        let rootfs = scratch.join("rootfs");
        fs::create_dir_all(rootfs.join("usr/lib")).expect("usr/lib");
        symlink("usr/lib", rootfs.join("lib")).expect("relative symlink");
        let dest = scratch.join("out");

        copy_dir(&rootfs, &dest).expect("copy should succeed");

        assert_eq!(
            fs::read_link(dest.join("lib")).expect("link"),
            PathBuf::from("usr/lib")
        );
    }

    #[test]
    fn copy_dir_keeps_file_and_directory_modes() {
        let scratch = TempDir::new("gaia-starting-point-copy");
        let rootfs = scratch.join("rootfs");
        fs::create_dir_all(rootfs.join("secure")).expect("secure dir");
        fs::write(rootfs.join("secure/key"), "k").expect("file");
        fs::set_permissions(rootfs.join("secure/key"), fs::Permissions::from_mode(0o640))
            .expect("file mode");
        fs::set_permissions(rootfs.join("secure"), fs::Permissions::from_mode(0o750))
            .expect("dir mode");
        let dest = scratch.join("out");

        copy_dir(&rootfs, &dest).expect("copy should succeed");

        let file_mode = fs::metadata(dest.join("secure/key")).expect("file").mode();
        let dir_mode = fs::metadata(dest.join("secure")).expect("dir").mode();
        assert_eq!(file_mode & 0o7777, 0o640);
        assert_eq!(dir_mode & 0o7777, 0o750);
    }

    #[test]
    fn copy_dir_keeps_hardlinks_inside_the_tree() {
        let scratch = TempDir::new("gaia-starting-point-copy");
        let rootfs = scratch.join("rootfs");
        fs::create_dir_all(&rootfs).expect("rootfs");
        fs::write(rootfs.join("a"), "shared").expect("file");
        fs::hard_link(rootfs.join("a"), rootfs.join("b")).expect("hardlink");
        let dest = scratch.join("out");

        copy_dir(&rootfs, &dest).expect("copy should succeed");

        let a = fs::metadata(dest.join("a")).expect("a");
        let b = fs::metadata(dest.join("b")).expect("b");
        assert_eq!((a.dev(), a.ino()), (b.dev(), b.ino()));
    }
}
