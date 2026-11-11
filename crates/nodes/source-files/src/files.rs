//! Copying a directory as it is, and normalizing modes.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Copies `from` into `to` (which must not exist): files with their modes,
/// directories, symlinks as links. Anything else is an error.
pub(crate) fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&src)?, &dst)?;
        } else if kind.is_dir() {
            copy_tree(&src, &dst)?;
            fs::set_permissions(&dst, fs::metadata(&src)?.permissions())?;
        } else if kind.is_file() {
            fs::copy(&src, &dst)?;
        } else {
            return Err(io::Error::other(format!(
                "{} is not a file, directory or symlink",
                src.display()
            )));
        }
    }
    Ok(())
}

/// Files 0644 or 0755 (any execute bit), directories 0755.
pub(crate) fn normalize_modes(dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
            normalize_modes(&path)?;
        } else {
            let mode = entry.metadata()?.permissions().mode();
            let mode = if mode & 0o111 != 0 { 0o755 } else { 0o644 };
            fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
        }
    }
    Ok(())
}
