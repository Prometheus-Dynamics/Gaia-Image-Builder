//! Reading a stored tree for the checks: its paths, and what a path reaches
//! when symbolic links are followed inside the tree.
//!
//! A link's target is read from the tree's root when it is absolute, so an
//! absolute link never reaches the build host. Nothing in the tree is written.

use std::collections::VecDeque;
use std::fs;
use std::path::Path;

/// How many links one lookup follows before it gives up.
const MAX_LINKS: usize = 40;

/// What a path of a tree is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
    Link,
}

/// Every path under `root`, relative and `/`-separated, sorted by path
/// component so a directory sorts before what is under it. Symbolic links
/// are listed, not followed. Anything that is not a file, directory or
/// symlink is an error.
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
        let kind = item.file_type().map_err(|e| format!("{relative}: {e}"))?;
        if kind.is_symlink() {
            out.push((relative, Kind::Link));
        } else if kind.is_dir() {
            out.push((relative.clone(), Kind::Dir));
            walk_into(&item.path(), &relative, out)?;
        } else if kind.is_file() {
            out.push((relative, Kind::File));
        } else {
            return Err(format!("{relative} is not a file, directory or symlink"));
        }
    }
    Ok(())
}

/// The path `rel` reaches under `root`, with every symbolic link on the way
/// followed inside the tree. `None` when a part of it is missing, a link
/// dangles, or the links loop. The result names no symlink.
pub fn resolve(root: &Path, rel: &str) -> Option<String> {
    let mut pending: VecDeque<String> = rel.split('/').map(str::to_string).collect();
    let mut done: Vec<String> = Vec::new();
    let mut links = 0;
    while let Some(part) = pending.pop_front() {
        match part.as_str() {
            "" | "." => continue,
            ".." => {
                done.pop();
                continue;
            }
            _ => {}
        }
        let mut path = root.to_path_buf();
        for piece in &done {
            path.push(piece);
        }
        path.push(&part);
        let meta = fs::symlink_metadata(&path).ok()?;
        if meta.file_type().is_symlink() {
            links += 1;
            if links > MAX_LINKS {
                return None;
            }
            let target = fs::read_link(&path).ok()?;
            let target = target.to_str()?.to_string();
            if target.starts_with('/') {
                done.clear();
            }
            for piece in target.split('/').rev() {
                pending.push_front(piece.to_string());
            }
        } else {
            done.push(part);
        }
    }
    Some(done.join("/"))
}

/// Whether `rel` is a regular file under `root` (following links inside it).
pub fn is_file(root: &Path, rel: &str) -> bool {
    resolve(root, rel).is_some_and(|found| root.join(found).is_file())
}

/// The bytes of the regular file `rel` reaches under `root`.
pub fn read(root: &Path, rel: &str) -> Option<Vec<u8>> {
    let found = resolve(root, rel)?;
    let path = root.join(found);
    path.is_file().then(|| fs::read(path).ok()).flatten()
}
