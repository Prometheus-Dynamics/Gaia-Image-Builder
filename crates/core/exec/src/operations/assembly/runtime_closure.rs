//! Copying a dynamically linked binary's runtime closure into a tree: the
//! interpreter and libraries the sysroot holds, at their absolute paths, with
//! the links crossed on the way. Shared by the BusyBox initramfs and the
//! listed binaries.

use super::*;
use gaia_plan::RuntimeEntry;

/// Whether a binary needed its runtime closure copied, and how it links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuntimeLinkage {
    NotRequested,
    Static,
    Dynamic,
}

impl RuntimeLinkage {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::NotRequested => "not-requested",
            Self::Static => "static",
            Self::Dynamic => "dynamic",
        }
    }
}

/// Writes a resolved runtime closure into the tree at its absolute paths:
/// the interpreter and each library as a copy of the sysroot's file, and each
/// symlink crossed on the way as a symlink with the sysroot's target text.
/// Returns the tree paths of the copied files.
pub(super) fn copy_runtime_closure(
    tree: &Path,
    closure: &gaia_plan::RuntimeClosure,
) -> Result<Vec<PathBuf>, String> {
    let mut copied = Vec::new();
    for entry in &closure.entries {
        let dest = tree.join(entry.guest().trim_start_matches('/'));
        if let Some(parent) = dest.parent() {
            std_fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create runtime dir '{}': {error}",
                    parent.display()
                )
            })?;
        }
        if let Ok(metadata) = dest.symlink_metadata() {
            if metadata.is_dir() {
                return Err(format!(
                    "runtime path '{}' is a directory in the tree, but the sysroot needs it as a link or file; do not create that directory in the tree",
                    dest.display()
                ));
            }
            std_fs::remove_file(&dest).map_err(|error| {
                format!(
                    "failed to replace runtime path '{}': {error}",
                    dest.display()
                )
            })?;
        }
        match entry {
            RuntimeEntry::File { source, .. } => {
                std_fs::copy(source, &dest).map_err(|error| {
                    format!(
                        "failed to copy runtime file '{}' to '{}': {error}",
                        source.display(),
                        dest.display()
                    )
                })?;
                copied.push(dest);
            }
            RuntimeEntry::Symlink { target, .. } => create_runtime_symlink(target, &dest)?,
        }
    }
    Ok(copied)
}

#[cfg(unix)]
fn create_runtime_symlink(target: &str, dest: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(target, dest).map_err(|error| {
        format!(
            "failed to create busybox runtime link '{}' -> '{target}': {error}",
            dest.display()
        )
    })
}

#[cfg(not(unix))]
fn create_runtime_symlink(_target: &str, dest: &Path) -> Result<(), String> {
    Err(format!(
        "runtime link '{}' needs a Unix host to create symlinks",
        dest.display()
    ))
}
