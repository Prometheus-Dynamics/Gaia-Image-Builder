//! Per-build packing of root filesystem images and post-image scripts, run
//! against the private target and images dirs of a shared-tree view.

use super::*;

/// Packs each root filesystem image from the private target using the
/// fakeroot script Buildroot generated for the shared tree. Each pack works
/// on its own clone of the target (as Buildroot does), so the fakeroot
/// device and user setup never touches the private target itself.
pub(crate) fn pack_private_rootfs(
    shared_dir: &Path,
    output_dir: &Path,
    buildroot_dir: &Path,
    fs_types: &[&str],
    command: ImageCommandContext<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    let mut messages = Vec::new();
    let pack_dir = output_dir.join(PRIVATE_PACK_DIR);
    let host_dir = shared_dir.join("host");
    let images_dir = output_dir.join("images");
    fs::create_dir_all(&images_dir).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!("failed to create '{}': {error}", images_dir.display()),
        )
    })?;
    for fs_type in fs_types {
        let fs_dir = shared_dir.join("build/buildroot-fs").join(fs_type);
        let script = fs::read_to_string(fs_dir.join("fakeroot")).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::OutputMissing,
                format!(
                    "shared Buildroot tree has no fakeroot script for '{fs_type}' at '{}': {error}",
                    fs_dir.display()
                ),
            )
        })?;
        let work_dir = pack_dir.join(fs_type);
        let work_target = work_dir.join("target");
        clone_tree(&output_dir.join("target"), &work_target)?;
        let rewritten = script
            .replace(
                &fs_dir.join("target").display().to_string(),
                &work_target.display().to_string(),
            )
            .replace(
                &shared_dir.join("images").display().to_string(),
                &images_dir.display().to_string(),
            );
        let work_script = work_dir.join("fakeroot");
        write_small_file(&work_script, &rewritten)?;
        #[cfg(unix)]
        fs::set_permissions(&work_script, fs::Permissions::from_mode(0o755)).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to mark '{}' executable: {error}",
                    work_script.display()
                ),
            )
        })?;
        let mut pack = Command::new(host_dir.join("bin/fakeroot"));
        pack.arg("--")
            // Through sh, so a just-written script never hits ETXTBSY.
            .arg("/bin/sh")
            .arg(&work_script)
            .current_dir(buildroot_dir)
            .env("PATH", host_tool_path(&host_dir)?)
            .env("FAKEROOTDONTTRYCHOWN", "1");
        let result = run_command(
            pack,
            &format!("buildroot {fs_type} image pack"),
            command.execution,
            command.policy,
            command.log_sink.clone(),
            command.cancel_check.clone(),
        );
        remove_path_if_exists(&work_dir)?;
        messages.extend(result?);
        messages.push(format!(
            "packed {fs_type} root filesystem from the private target"
        ));
    }
    remove_path_if_exists(&pack_dir)?;
    Ok(messages)
}

fn host_tool_path(host_dir: &Path) -> Result<std::ffi::OsString, ImageProviderError> {
    let mut entries = vec![host_dir.join("bin"), host_dir.join("sbin")];
    entries.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    env::join_paths(entries).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!("failed to build PATH for Buildroot host tools: {error}"),
        )
    })
}

/// Runs the configured post-image scripts the way Buildroot does, with the
/// build's private images and target directories.
pub(crate) fn run_private_post_image(
    shared_dir: &Path,
    output_dir: &Path,
    buildroot_dir: &Path,
    command: ImageCommandContext<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    let (scripts, args) = post_image_settings(shared_dir, buildroot_dir, command.clone());
    let mut messages = Vec::new();
    let host_dir = shared_dir.join("host");
    let images_dir = output_dir.join("images");
    for script in scripts {
        let script_path = if Path::new(&script).is_absolute() {
            PathBuf::from(&script)
        } else {
            buildroot_dir.join(&script)
        };
        let mut post_image = Command::new(&script_path);
        post_image
            .arg(&images_dir)
            .args(args.split_whitespace())
            .current_dir(buildroot_dir)
            .env("PATH", host_tool_path(&host_dir)?)
            .env("BR2_CONFIG", shared_dir.join(".config"))
            .env("HOST_DIR", &host_dir)
            .env("STAGING_DIR", shared_dir.join("staging"))
            .env("TARGET_DIR", output_dir.join("target"))
            .env("BUILD_DIR", shared_dir.join("build"))
            .env("BINARIES_DIR", &images_dir)
            .env("BASE_DIR", shared_dir);
        messages.extend(run_command(
            post_image,
            "buildroot post-image script",
            command.execution,
            command.policy,
            command.log_sink.clone(),
            command.cancel_check.clone(),
        )?);
        messages.push(format!("ran post-image script '{script}'"));
    }
    Ok(messages)
}

/// The post-image scripts and their arguments, with make variables such as
/// `$(BR2_EXTERNAL_<NAME>_PATH)` expanded by Buildroot's `printvars`. Falls
/// back to the raw `.config` values.
fn post_image_settings(
    shared_dir: &Path,
    buildroot_dir: &Path,
    command: ImageCommandContext<'_>,
) -> (Vec<String>, String) {
    let config = fs::read_to_string(shared_dir.join(".config")).unwrap_or_default();
    let mut scripts =
        buildroot_config_value(&config, "BR2_ROOTFS_POST_IMAGE_SCRIPT").unwrap_or_default();
    let mut args =
        buildroot_config_value(&config, "BR2_ROOTFS_POST_SCRIPT_ARGS").unwrap_or_default();
    if scripts.trim().is_empty() {
        return (Vec::new(), args);
    }
    let mut printvars = Command::new("make");
    printvars
        .arg(format!("O={}", shared_dir.display()))
        .arg("-s")
        .arg("--no-print-directory")
        .arg("printvars")
        .arg("VARS=BR2_ROOTFS_POST_IMAGE_SCRIPT BR2_ROOTFS_POST_SCRIPT_ARGS")
        .current_dir(buildroot_dir);
    if let Ok(output) = command_output_with_timeout(
        &mut printvars,
        command.execution,
        Duration::from_secs(command.policy.timeout_seconds.max(60)),
        "buildroot printvars",
        command.policy.output_retention,
        None,
        command.cancel_check.clone(),
    ) && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if let Some(value) = line.strip_prefix("BR2_ROOTFS_POST_IMAGE_SCRIPT=") {
                scripts = unquote_value(value).to_string();
            } else if let Some(value) = line.strip_prefix("BR2_ROOTFS_POST_SCRIPT_ARGS=") {
                args = unquote_value(value).to_string();
            }
        }
    }
    (
        scripts.split_whitespace().map(str::to_string).collect(),
        args,
    )
}

fn unquote_value(value: &str) -> &str {
    let value = value.trim();
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
}
