//! Walking and copying a stored tree: its paths, sorted, and a copy of it
//! that a pass may change. Modes are kept; symlinks stay links.

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

/// What a path of a tree is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
    Link,
}

/// Every path under `root`, relative and `/`-separated, sorted by path
/// component so a directory sorts before what is under it. Anything that is
/// not a file, directory or symlink is an error.
pub fn walk(root: &Path) -> Result<Vec<(String, Kind)>, String> {
    let mut found = Vec::new();
    walk_into(root, "", &mut found)?;
    found.sort_by(|a, b| a.0.split('/').cmp(b.0.split('/')));
    Ok(found)
}

fn walk_into(dir: &Path, prefix: &str, out: &mut Vec<(String, Kind)>) -> Result<(), String> {
    for item in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let item = item.map_err(|e| format!("{}: {e}", dir.display()))?;
        let name = item
            .file_name()
            .into_string()
            .map_err(|n| format!("{}: not UTF-8", Path::new(&n).display()))?;
        let relative = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let path = item.path();
        let kind = item.file_type().map_err(|e| format!("{relative}: {e}"))?;
        if kind.is_symlink() {
            out.push((relative, Kind::Link));
        } else if kind.is_dir() {
            out.push((relative.clone(), Kind::Dir));
            walk_into(&path, &relative, out)?;
        } else if kind.is_file() {
            out.push((relative, Kind::File));
        } else {
            return Err(format!("{relative} is not a file, directory or symlink"));
        }
    }
    Ok(())
}

/// Copies the tree at `from` to `to`, which must not exist yet. Directory
/// modes are set after their contents are copied.
pub fn copy(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let mut dirs = Vec::new();
    for (relative, kind) in walk(from)? {
        let src = from.join(&relative);
        let dst = to.join(&relative);
        let fail = |e: std::io::Error| format!("{relative}: {e}");
        match kind {
            Kind::Dir => {
                fs::create_dir(&dst).map_err(fail)?;
                dirs.push((src, dst));
            }
            Kind::File => {
                fs::copy(&src, &dst).map_err(fail)?;
            }
            Kind::Link => {
                let target = fs::read_link(&src).map_err(fail)?;
                symlink(target, &dst).map_err(fail)?;
            }
        }
    }
    // Deepest first, so a read-only directory does not stop its children.
    for (src, dst) in dirs.iter().rev() {
        let mode = fs::metadata(src)
            .map_err(|e| format!("{}: {e}", src.display()))?
            .permissions()
            .mode();
        fs::set_permissions(dst, fs::Permissions::from_mode(mode & 0o7777))
            .map_err(|e| format!("{}: {e}", dst.display()))?;
    }
    Ok(())
}
