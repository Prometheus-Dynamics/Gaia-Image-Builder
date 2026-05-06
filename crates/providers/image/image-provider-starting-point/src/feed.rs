use super::*;

pub(crate) fn apply_image_feed_to_rootfs(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
    rootfs_dir: &Path,
) -> Result<(), ImageProviderError> {
    for install_id in &image.feed.install_entries {
        let install = spec
            .install
            .entries
            .iter()
            .find(|entry| entry.id == *install_id)
            .ok_or_else(|| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "image feed references unknown install '{}'",
                        install_id.as_str()
                    ),
                )
            })?;
        let artifact = spec
            .artifacts
            .iter()
            .find(|artifact| artifact.id == install.artifact.id)
            .ok_or_else(|| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "install '{}' references unknown artifact '{}'",
                        install.id.as_str(),
                        install.artifact.id.as_str()
                    ),
                )
            })?;
        let src = if Path::new(&artifact.output.path).is_absolute() {
            PathBuf::from(&artifact.output.path)
        } else {
            PathBuf::from(&spec.workspace.root_dir).join(&artifact.output.path)
        };
        if !src.exists() {
            return Err(ImageProviderError::new(
                ImageProviderErrorKind::OutputMissing,
                format!(
                    "install artifact output missing for '{}': {}",
                    artifact.id.as_str(),
                    src.display()
                ),
            ));
        }
        let dest = rootfs_path(rootfs_dir, &install.dest);
        copy_into_rootfs(&src, &dest)?;
        #[cfg(unix)]
        if let Some(mode) = install.mode {
            fs::set_permissions(&dest, fs::Permissions::from_mode(mode)).map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to set install mode on '{}': {error}",
                        dest.display()
                    ),
                )
            })?;
        }
    }

    for stage_file_id in &image.feed.stage_files {
        let stage_file = spec
            .stage
            .files
            .iter()
            .find(|file| file.id == *stage_file_id)
            .ok_or_else(|| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "image feed references unknown stage file '{}'",
                        stage_file_id.as_str()
                    ),
                )
            })?;
        let src = resolve_workspace_path(spec, &stage_file.src)?;
        let dest = rootfs_path(rootfs_dir, &stage_file.dest);
        copy_into_rootfs(&src, &dest)?;
        #[cfg(unix)]
        if let Some(mode) = stage_file.mode {
            fs::set_permissions(&dest, fs::Permissions::from_mode(mode)).map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to set stage file mode on '{}': {error}",
                        dest.display()
                    ),
                )
            })?;
        }
    }

    for env_set_id in &image.feed.stage_env_sets {
        let env_set = spec
            .stage
            .env_sets
            .iter()
            .find(|env_set| env_set.id == *env_set_id)
            .ok_or_else(|| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "image feed references unknown stage env set '{}'",
                        env_set_id.as_str()
                    ),
                )
            })?;
        let dest = rootfs_dir
            .join("etc")
            .join("default")
            .join(format!("{}.env", env_set.name));
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to create env set dir '{}': {error}",
                        parent.display()
                    ),
                )
            })?;
        }
        let mut contents = String::new();
        for (key, value) in &env_set.entries {
            contents.push_str(key);
            contents.push('=');
            contents.push_str(value);
            contents.push('\n');
        }
        fs::write(&dest, contents).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!("failed to write env set '{}': {error}", dest.display()),
            )
        })?;
    }

    for service_id in &image.feed.stage_services {
        let service = spec
            .stage
            .services
            .iter()
            .find(|service| service.id == *service_id)
            .ok_or_else(|| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "image feed references unknown stage service '{}'",
                        service_id.as_str()
                    ),
                )
            })?;
        let src = resolve_workspace_path(spec, &service.unit_path)?;
        let dest = rootfs_dir
            .join("etc")
            .join("systemd")
            .join("system")
            .join(&service.name);
        copy_into_rootfs(&src, &dest)?;
    }

    Ok(())
}

pub(crate) fn rootfs_path(rootfs_dir: &Path, image_path: &str) -> PathBuf {
    rootfs_dir.join(image_path.trim_start_matches('/'))
}

pub(crate) fn resolve_workspace_path(
    spec: &ResolvedBuildSpec,
    raw: &str,
) -> Result<PathBuf, ImageProviderError> {
    gaia_spec::resolve_workspace_path(&spec.workspace, raw).map_err(|error| {
        ImageProviderError::new(ImageProviderErrorKind::RuntimeState, error.to_string())
    })
}

pub(crate) fn copy_into_rootfs(src: &Path, dest: &Path) -> Result<(), ImageProviderError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to create destination dir '{}': {error}",
                    parent.display()
                ),
            )
        })?;
    }
    if src.is_dir() {
        copy_dir(src, dest)
    } else {
        fs::copy(src, dest).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to copy source '{}' to '{}': {error}",
                    src.display(),
                    dest.display()
                ),
            )
        })?;
        Ok(())
    }
}
