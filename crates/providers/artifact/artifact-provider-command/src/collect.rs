//! Copies a collected path to the artifact output.

use gaia_artifact_providers::{ArtifactProviderError, ArtifactProviderErrorKind};
use std::fs;
use std::path::{Component, Path, PathBuf};

fn runtime(message: String) -> ArtifactProviderError {
    ArtifactProviderError::new(ArtifactProviderErrorKind::RuntimeState, message)
}

/// Removes `path` (a file, a link, or a directory tree) when it exists.
pub(crate) fn remove_existing(path: &Path) -> Result<(), ArtifactProviderError> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    let result = if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(|error| {
        runtime(format!(
            "failed to remove the old output '{}': {error}",
            path.display()
        ))
    })
}

/// A relative path in its plain form: `./a//b/` is `a/b`.
fn normalize(path: &str) -> PathBuf {
    Path::new(path)
        .components()
        .filter(|component| matches!(component, Component::Normal(_)))
        .collect()
}

/// Copies `from` (a file or a directory) to `to` through a temporary path
/// beside it, leaving out the `exclude` paths (relative to `from`).
pub(crate) fn copy_collected(
    from: &Path,
    to: &Path,
    exclude: &[String],
) -> Result<(), ArtifactProviderError> {
    let excluded: Vec<PathBuf> = exclude.iter().map(|path| normalize(path)).collect();
    let temp = to.with_extension("gaia.collect.tmp");
    remove_existing(&temp)?;
    let copied = copy_tree(from, &temp, Path::new(""), &excluded);
    if let Err(error) = copied {
        let _ = remove_existing(&temp);
        return Err(error);
    }
    remove_existing(to)?;
    fs::rename(&temp, to).map_err(|error| {
        let _ = remove_existing(&temp);
        runtime(format!(
            "failed to move the collected output into place '{}': {error}",
            to.display()
        ))
    })
}

fn copy_tree(
    from: &Path,
    to: &Path,
    relative: &Path,
    excluded: &[PathBuf],
) -> Result<(), ArtifactProviderError> {
    let meta = fs::symlink_metadata(from)
        .map_err(|error| runtime(format!("failed to read '{}': {error}", from.display())))?;
    let file_type = meta.file_type();
    if file_type.is_symlink() {
        let target = fs::read_link(from).map_err(|error| {
            runtime(format!("failed to read link '{}': {error}", from.display()))
        })?;
        return symlink(&target, to);
    }
    if file_type.is_dir() {
        fs::create_dir_all(to)
            .map_err(|error| runtime(format!("failed to create '{}': {error}", to.display())))?;
        let mut entries = fs::read_dir(from)
            .map_err(|error| runtime(format!("failed to list '{}': {error}", from.display())))?
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect::<Vec<_>>();
        entries.sort();
        for name in entries {
            let child = relative.join(&name);
            if excluded.contains(&child) {
                continue;
            }
            copy_tree(&from.join(&name), &to.join(&name), &child, excluded)?;
        }
        return fs::set_permissions(to, meta.permissions()).map_err(|error| {
            runtime(format!("failed to set mode of '{}': {error}", to.display()))
        });
    }
    fs::copy(from, to).map(|_| ()).map_err(|error| {
        runtime(format!(
            "failed to copy '{}' to '{}': {error}",
            from.display(),
            to.display()
        ))
    })
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> Result<(), ArtifactProviderError> {
    std::os::unix::fs::symlink(target, link).map_err(|error| {
        runtime(format!(
            "failed to create link '{}': {error}",
            link.display()
        ))
    })
}

#[cfg(not(unix))]
fn symlink(target: &Path, link: &Path) -> Result<(), ArtifactProviderError> {
    let _ = (target, link);
    Err(runtime(format!(
        "cannot copy link '{}' on this platform",
        link.display()
    )))
}
