//! Reading a stored tree path by path, and copying one path out of it.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

/// What is at a path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Node {
    Dir,
    File,
    Link(PathBuf),
}

impl Node {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Dir => "a directory",
            Self::File => "a file",
            Self::Link(_) => "a symlink",
        }
    }
}

/// One path of a tree: what it is and its stored mode (0 for a symlink).
#[derive(Clone, Debug)]
pub(crate) struct Found {
    pub node: Node,
    pub mode: u32,
}

/// Every path under `root`, `/`-separated and relative, sorted.
pub(crate) fn list(root: &Path) -> io::Result<BTreeMap<String, Found>> {
    let mut found = BTreeMap::new();
    walk(root, "", &mut found)?;
    Ok(found)
}

fn walk(dir: &Path, prefix: &str, found: &mut BTreeMap<String, Found>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|name| {
            io::Error::other(format!("{}: not UTF-8", Path::new(&name).display()))
        })?;
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let kind = entry.file_type()?;
        let mode = if kind.is_symlink() {
            0
        } else {
            entry.metadata()?.permissions().mode() & 0o7777
        };
        let node = if kind.is_symlink() {
            Node::Link(fs::read_link(entry.path())?)
        } else if kind.is_dir() {
            Node::Dir
        } else if kind.is_file() {
            Node::File
        } else {
            return Err(io::Error::other(format!(
                "{path} is not a file, directory or symlink"
            )));
        };
        let is_dir = node == Node::Dir;
        found.insert(path.clone(), Found { node, mode });
        if is_dir {
            walk(&entry.path(), &path, found)?;
        }
    }
    Ok(())
}

/// `path` as a relative path inside a tree: no `..`, no empty name. A
/// leading `/` is dropped.
pub(crate) fn inside(path: &str) -> Result<String, String> {
    let trimmed = path.trim_start_matches('/');
    let ok = !trimmed.is_empty()
        && Path::new(trimmed)
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
    if ok {
        Ok(trimmed.trim_end_matches('/').to_string())
    } else {
        Err(format!("{path} is not a path inside the tree"))
    }
}

/// Copies one path (a file, a symlink, or a directory and all it holds)
/// from `from` to `to`, keeping modes. `to`'s parent must exist.
pub(crate) fn copy(from: &Path, to: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(from)?, to)
    } else if meta.is_dir() {
        fs::create_dir(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy(&entry.path(), &to.join(entry.file_name()))?;
        }
        fs::set_permissions(to, meta.permissions())
    } else {
        fs::copy(from, to)?;
        fs::set_permissions(to, meta.permissions())
    }
}

/// Makes `dir` and any missing parents, each 0755.
pub(crate) fn make_dirs(dir: &Path) -> io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    if let Some(parent) = dir.parent() {
        make_dirs(parent)?;
    }
    fs::create_dir(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o755))
}

/// Sets a file's or directory's permission bits.
pub(crate) fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777))
}
