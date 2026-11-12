//! Copies a tree of files for a step's scratch space: directories, files
//! with their modes, and symlinks as links.

use std::fs;
use std::io;
use std::path::Path;

/// Copies `from` to `to`, which must not exist.
pub fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&src)?, &dst)?;
        } else if kind.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            fs::copy(&src, &dst)?;
        }
    }
    fs::set_permissions(to, fs::metadata(from)?.permissions())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn links_and_modes_are_kept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let from = dir.path().join("from");
        fs::create_dir_all(from.join("sub")).expect("mkdir");
        fs::write(from.join("sub/tool"), "x").expect("write");
        fs::set_permissions(from.join("sub/tool"), fs::Permissions::from_mode(0o750))
            .expect("chmod");
        std::os::unix::fs::symlink("tool", from.join("sub/alias")).expect("link");
        let to = dir.path().join("to");
        copy_tree(&from, &to).expect("copy");
        assert_eq!(
            fs::read_link(to.join("sub/alias")).expect("link"),
            Path::new("tool")
        );
        let mode = fs::metadata(to.join("sub/tool"))
            .expect("meta")
            .permissions();
        assert_eq!(mode.mode() & 0o777, 0o750);
    }
}
