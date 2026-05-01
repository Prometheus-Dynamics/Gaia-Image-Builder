use super::*;

pub(crate) fn rootfs_path(rootfs_dir: &Path, image_path: &str) -> PathBuf {
    let trimmed = image_path.trim_start_matches('/');
    rootfs_dir.join(trimmed)
}

pub(crate) fn resolve_workspace_path(
    spec: &ResolvedBuildSpec,
    raw: &str,
) -> Result<PathBuf, ImageProviderError> {
    gaia_spec::resolve_workspace_path(&spec.workspace, raw).map_err(|error| {
        ImageProviderError::new(ImageProviderErrorKind::RuntimeState, error.to_string())
    })
}

pub(crate) fn copy_path(src: &Path, dest: &Path) -> Result<(), ImageProviderError> {
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
    let metadata = fs::symlink_metadata(src).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!("failed to stat source '{}': {error}", src.display()),
        )
    })?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(src).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!("failed to read symlink '{}': {error}", src.display()),
            )
        })?;
        if let Ok(dest_meta) = fs::symlink_metadata(dest) {
            if dest_meta.is_dir() {
                fs::remove_dir_all(dest).map_err(|error| {
                    ImageProviderError::new(
                        ImageProviderErrorKind::RuntimeState,
                        format!(
                            "failed to remove existing destination dir '{}': {error}",
                            dest.display()
                        ),
                    )
                })?;
            } else {
                fs::remove_file(dest).map_err(|error| {
                    ImageProviderError::new(
                        ImageProviderErrorKind::RuntimeState,
                        format!(
                            "failed to remove existing destination file '{}': {error}",
                            dest.display()
                        ),
                    )
                })?;
            }
        } else if dest.exists() {
            let dest_meta = fs::symlink_metadata(dest).map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to stat existing destination '{}': {error}",
                        dest.display()
                    ),
                )
            })?;
            if dest_meta.is_dir() {
                fs::remove_dir_all(dest).map_err(|error| {
                    ImageProviderError::new(
                        ImageProviderErrorKind::RuntimeState,
                        format!(
                            "failed to remove existing destination dir '{}': {error}",
                            dest.display()
                        ),
                    )
                })?;
            } else {
                fs::remove_file(dest).map_err(|error| {
                    ImageProviderError::new(
                        ImageProviderErrorKind::RuntimeState,
                        format!(
                            "failed to remove existing destination file '{}': {error}",
                            dest.display()
                        ),
                    )
                })?;
            }
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, dest).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to copy symlink '{}' to '{}': {error}",
                    src.display(),
                    dest.display()
                ),
            )
        })?;
        #[cfg(not(unix))]
        {
            return Err(ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to copy symlink '{}' to '{}': symlink copy is unsupported on this platform",
                    src.display(),
                    dest.display()
                ),
            ));
        }
        return Ok(());
    }
    if metadata.is_dir() {
        fs::create_dir_all(dest).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!("failed to create directory '{}': {error}", dest.display()),
            )
        })?;
        for entry in fs::read_dir(src).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!("failed to read directory '{}': {error}", src.display()),
            )
        })? {
            let entry = entry.map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to read directory entry in '{}': {error}",
                        src.display()
                    ),
                )
            })?;
            copy_path(&entry.path(), &dest.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if let Ok(dest_meta) = fs::symlink_metadata(dest) {
        if dest_meta.is_dir() {
            fs::remove_dir_all(dest).map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to remove existing destination dir '{}': {error}",
                        dest.display()
                    ),
                )
            })?;
        } else {
            fs::remove_file(dest).map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to remove existing destination file '{}': {error}",
                        dest.display()
                    ),
                )
            })?;
        }
    }
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

pub(crate) fn merge_tree_contents(
    src_dir: &Path,
    dest_dir: &Path,
) -> Result<(), ImageProviderError> {
    fs::create_dir_all(dest_dir).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to create destination dir '{}': {error}",
                dest_dir.display()
            ),
        )
    })?;

    let entries = fs::read_dir(src_dir).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!("failed to read source dir '{}': {error}", src_dir.display()),
        )
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to read entry under '{}': {error}",
                    src_dir.display()
                ),
            )
        })?;
        copy_path(&entry.path(), &dest_dir.join(entry.file_name()))?;
    }

    Ok(())
}
