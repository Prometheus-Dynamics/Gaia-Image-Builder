//! File-level helpers of the package cache: walking per-package trees,
//! cloning entries in and out, finding the output path in files, and the
//! order packages and their stamps are restored in.
use super::*;
use std::collections::HashMap;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::time::UNIX_EPOCH;

/// The cache key a package's build directory was made or restored with.
const PACKAGE_KEY_FILE: &str = ".gaia-package-key";

pub(crate) fn record_package_key(stamp_dir: &Path, key: &str) {
    let _ = fs::write(stamp_dir.join(PACKAGE_KEY_FILE), key);
}

/// Makes the stamps of every built package whose inputs are unchanged (its
/// recorded key is its current key) newer than anything they depend on,
/// keeping their order. A key covers the content of everything a package's
/// steps read, so a newer modification time on unchanged content (sources
/// copied again, for example) must not make `make` redo steps: for a
/// restored package that has no sources to redo them from, it would fail.
pub(crate) fn refresh_current_stamps(
    output_dir: &Path,
    graph: &PackageGraph,
    keys: &BTreeMap<String, Option<String>>,
) {
    let now = std::time::SystemTime::now();
    for name in graph.package_names() {
        let (Some(Some(key)), Some(stamp_dir)) = (
            keys.get(name),
            graph
                .get(name)
                .and_then(|package| package.stamp_dir.as_deref()),
        ) else {
            continue;
        };
        let stamp_dir = output_dir.join(stamp_dir);
        let recorded = fs::read_to_string(stamp_dir.join(PACKAGE_KEY_FILE)).unwrap_or_default();
        if recorded.trim() != key || !stamp_dir.join(".stamp_installed").is_file() {
            continue;
        }
        for (index, stamp) in stamps_in(&stamp_dir).iter().enumerate() {
            if let Ok(file) = fs::File::options().append(true).open(stamp_dir.join(stamp)) {
                let _ = file.set_modified(now + Duration::from_millis(10 * index as u64));
            }
        }
    }
}

/// Package names, each after all its dependencies.
pub(crate) fn dependency_order(graph: &PackageGraph) -> Vec<String> {
    fn visit(
        graph: &PackageGraph,
        name: &str,
        seen: &mut BTreeSet<String>,
        order: &mut Vec<String>,
    ) {
        if !seen.insert(name.to_string()) {
            return;
        }
        if let Some(package) = graph.get(name) {
            for dependency in &package.dependencies {
                visit(graph, dependency, seen, order);
            }
            order.push(name.to_string());
        }
    }
    let mut seen = BTreeSet::new();
    let mut order = Vec::new();
    for name in graph.package_names() {
        visit(graph, name, &mut seen, &mut order);
    }
    order
}

/// Every package `name` depends on, directly or not.
pub(crate) fn recursive_dependencies(graph: &PackageGraph, name: &str) -> BTreeSet<String> {
    let mut all = BTreeSet::new();
    let mut queue = graph
        .get(name)
        .map(|package| package.dependencies.iter().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    while let Some(dependency) = queue.pop() {
        if all.insert(dependency.clone())
            && let Some(package) = graph.get(&dependency)
        {
            queue.extend(package.dependencies.iter().cloned());
        }
    }
    all
}

/// Calls `visit` with the path (relative to `output_dir`) and metadata of
/// every file and symlink under `relative`.
pub(crate) fn walk_files(
    output_dir: &Path,
    relative: &str,
    visit: &mut dyn FnMut(String, fs::Metadata),
) {
    let Ok(entries) = fs::read_dir(output_dir.join(relative)) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let member = format!("{relative}/{name}");
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if metadata.is_dir() {
            walk_files(output_dir, &member, visit);
        } else {
            visit(member, metadata);
        }
    }
}

pub(crate) fn collect_dirs(output_dir: &Path, relative: &str, members: &mut BTreeSet<String>) {
    let Ok(entries) = fs::read_dir(output_dir.join(relative)) else {
        return;
    };
    members.insert(relative.to_string());
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir())
            && let Some(name) = entry.file_name().to_str()
        {
            collect_dirs(output_dir, &format!("{relative}/{name}"), members);
        }
    }
}

/// A package's stamps in the order its build created them (by modification
/// time, then Buildroot's usual order).
pub(crate) fn stamps_in(dir: &Path) -> Vec<String> {
    let mut stamps = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            let modified = entry.metadata().ok()?.modified().ok()?;
            name.starts_with(".stamp_").then_some((modified, name))
        })
        .collect::<Vec<_>>();
    stamps.sort_by_key(|(modified, name)| {
        (
            *modified,
            STAMP_ORDER
                .iter()
                .position(|known| known == name)
                .unwrap_or(STAMP_ORDER.len() - 1),
            name.clone(),
        )
    });
    stamps.into_iter().map(|(_, name)| name).collect()
}

/// Copies `dirs` (created with their modes) and `files` (cloned where the
/// filesystem allows, replacing what is there, never writing through a
/// hard link) from one root to another; paths are relative to both roots.
pub(crate) fn clone_members(
    from: &Path,
    to: &Path,
    dirs: &[(String, u32)],
    files: &[String],
) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|error| error.to_string())?;
    for (dir, _) in dirs {
        fs::create_dir_all(to.join(dir)).map_err(|error| format!("{dir}: {error}"))?;
    }
    for batch in files.chunks(256) {
        let output = Command::new("cp")
            .current_dir(from)
            .arg("-a")
            .arg("--reflink=auto")
            .arg("--remove-destination")
            .arg("--parents")
            .arg("-t")
            .arg(to)
            .arg("--")
            .args(batch)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| format!("cp: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "cp exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
                    .lines()
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(" | ")
            ));
        }
    }
    for (dir, mode) in dirs {
        let _ = fs::set_permissions(to.join(dir), fs::Permissions::from_mode(*mode));
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryKind {
    Directory,
    File,
    Symlink,
}

/// One entry of a dependency's per-package tree, as the link step needs it.
pub(crate) struct TreeEntry {
    /// Path relative to the tree root (empty for the root itself).
    relative: Box<str>,
    kind: EntryKind,
    /// Permissions and modification time (seconds, nanoseconds) of
    /// a file or directory. Symlinks are recreated from their targets.
    mode: u32,
    modified: (i64, i64),
}

impl TreeEntry {
    fn of(relative: String, kind: EntryKind, metadata: &fs::Metadata) -> Self {
        Self {
            relative: relative.into_boxed_str(),
            kind,
            mode: metadata.mode() & 0o7777,
            modified: (metadata.mtime(), metadata.mtime_nsec()),
        }
    }
}

/// Dependency trees listed once per restore, so each is walked once rather
/// than once for every package that depends on it.
#[derive(Default)]
pub(crate) struct TreeListings {
    trees: HashMap<PathBuf, Vec<TreeEntry>>,
}

impl TreeListings {
    /// Builds `destination` from the per-package trees `sources`, in
    /// dependency order, as `rsync -a --link-dest` run once per source
    /// would: see [`link_trees`].
    pub(crate) fn link(&mut self, destination: &Path, sources: &[PathBuf]) -> Result<(), String> {
        for source in sources {
            if !self.trees.contains_key(source) {
                let entries = list_tree(source)?;
                self.trees.insert(source.clone(), entries);
            }
        }
        let trees = sources
            .iter()
            .map(|source| (source.as_path(), self.trees[source].as_slice()))
            .collect::<Vec<_>>();
        link_trees(destination, &trees)
    }
}

fn list_tree(root: &Path) -> Result<Vec<TreeEntry>, String> {
    let metadata = fs::metadata(root).map_err(|error| format!("{}: {error}", root.display()))?;
    let mut entries = vec![TreeEntry::of(
        String::new(),
        EntryKind::Directory,
        &metadata,
    )];
    list_children(root, "", &mut entries)?;
    Ok(entries)
}

fn list_children(
    directory: &Path,
    relative: &str,
    entries: &mut Vec<TreeEntry>,
) -> Result<(), String> {
    let listing =
        fs::read_dir(directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    for entry in listing {
        let entry = entry.map_err(|error| format!("{}: {error}", directory.display()))?;
        let path = entry.path();
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| format!("{}: name is not UTF-8", path.display()))?;
        let member = if relative.is_empty() {
            name
        } else {
            format!("{relative}/{name}")
        };
        let kind = entry
            .file_type()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if kind.is_symlink() {
            entries.push(TreeEntry {
                relative: member.into_boxed_str(),
                kind: EntryKind::Symlink,
                mode: 0,
                modified: (0, 0),
            });
            continue;
        }
        let metadata = entry
            .metadata()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if kind.is_dir() {
            entries.push(TreeEntry::of(
                member.clone(),
                EntryKind::Directory,
                &metadata,
            ));
            list_children(&path, &member, entries)?;
        } else if kind.is_file() {
            entries.push(TreeEntry::of(member, EntryKind::File, &metadata));
        } else {
            return Err(format!(
                "{}: not a file, directory or symlink",
                path.display()
            ));
        }
    }
    Ok(())
}

/// Builds `destination` from `sources` (per-package trees with their
/// listings, in dependency order) the way `rsync -a --link-dest=<source>/`
/// run once per source does, without a process per source. For each path,
/// the last source holding it decides, as with rsync (which relinks a file
/// to the last source's copy even when size and time match an earlier one):
///
/// - a file is a hard link to the file at the same path in that source;
/// - a symlink is recreated with the same target (rsync also copies the
///   link's own time, which is not restored here);
/// - a directory takes the permissions and time of that source.
///
/// A path that is a directory in one source and not in another is an
/// error: rsync cannot replace it without deleting, so the package is built
/// instead.
fn link_trees(destination: &Path, sources: &[(&Path, &[TreeEntry])]) -> Result<(), String> {
    fs::create_dir_all(destination)
        .map_err(|error| format!("{}: {error}", destination.display()))?;
    let mut chosen: HashMap<&str, (usize, &TreeEntry)> = HashMap::new();
    for (index, (_, entries)) in sources.iter().enumerate() {
        for entry in entries.iter() {
            let key: &str = &entry.relative;
            if let Some(&(_, existing)) = chosen.get(key)
                && (existing.kind == EntryKind::Directory) != (entry.kind == EntryKind::Directory)
            {
                return Err(format!(
                    "{key}: a directory in one dependency and not in another"
                ));
            }
            chosen.insert(key, (index, entry));
        }
    }
    let mut order = chosen
        .into_iter()
        .map(|(key, (index, entry))| (key, index, entry))
        .collect::<Vec<_>>();
    order.sort_unstable_by_key(|(key, _, _)| *key);
    // Sorted, so a directory comes before its contents; applied in reverse
    // so contents are settled before the directory's own mode is.
    let mut directories = Vec::new();
    for (key, index, entry) in order {
        let root = sources[index].0;
        let (from, to) = if key.is_empty() {
            (root.to_path_buf(), destination.to_path_buf())
        } else {
            (root.join(key), destination.join(key))
        };
        match entry.kind {
            EntryKind::Directory => {
                if !key.is_empty() {
                    make_directory(&to)?;
                }
                directories.push((to, entry.mode, entry.modified));
            }
            EntryKind::File => link_file(&from, &to)?,
            EntryKind::Symlink => link_symlink(&from, &to)?,
        }
    }
    for (path, mode, modified) in directories.iter().rev() {
        apply_directory(path, *mode, *modified)?;
    }
    Ok(())
}

fn make_directory(path: &Path) -> Result<(), String> {
    let existing = fs::symlink_metadata(path);
    if existing.as_ref().is_ok_and(fs::Metadata::is_dir) {
        return Ok(());
    }
    if existing.is_ok() {
        fs::remove_file(path).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    fs::create_dir(path).map_err(|error| format!("{}: {error}", path.display()))
}

fn link_file(from: &Path, to: &Path) -> Result<(), String> {
    match fs::hard_link(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => fs::remove_file(to)
            .and_then(|()| fs::hard_link(from, to))
            .map_err(|error| format!("{}: {error}", to.display())),
        Err(error) => Err(format!("{}: {error}", to.display())),
    }
}

fn link_symlink(from: &Path, to: &Path) -> Result<(), String> {
    let target = fs::read_link(from).map_err(|error| format!("{}: {error}", from.display()))?;
    match std::os::unix::fs::symlink(&target, to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => fs::remove_file(to)
            .and_then(|()| std::os::unix::fs::symlink(&target, to))
            .map_err(|error| format!("{}: {error}", to.display())),
        Err(error) => Err(format!("{}: {error}", to.display())),
    }
}

fn apply_directory(path: &Path, mode: u32, modified: (i64, i64)) -> Result<(), String> {
    let display = path.display();
    if let Ok(seconds) = u64::try_from(modified.0) {
        let nanoseconds = u32::try_from(modified.1).unwrap_or(0);
        let time = UNIX_EPOCH + Duration::new(seconds, nanoseconds);
        fs::File::open(path)
            .and_then(|directory| directory.set_modified(time))
            .map_err(|error| format!("{display}: {error}"))?;
    }
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| format!("{display}: {error}"))
}

/// Whether a file contains `needle`, and in what kind of content.
pub(crate) enum Holds {
    No,
    Text,
    Binary,
}

/// Searches a file for `needle` in chunks, without reading it whole.
pub(crate) fn file_holds(path: &Path, needle: &[u8]) -> std::io::Result<Holds> {
    use std::io::Read;
    let mut file = fs::File::open(path)?;
    let mut buffer = vec![0u8; 1 << 20];
    let mut carry = Vec::new();
    let (mut found, mut binary) = (false, false);
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let chunk = &buffer[..read];
        binary |= chunk.contains(&0);
        if !found {
            carry.extend_from_slice(chunk);
            found = find_bytes(&carry, needle).is_some();
            let keep = needle.len().saturating_sub(1).min(carry.len());
            carry.drain(..carry.len() - keep);
        }
        if found && binary {
            break;
        }
    }
    Ok(match (found, binary) {
        (false, _) => Holds::No,
        (true, false) => Holds::Text,
        (true, true) => Holds::Binary,
    })
}

pub(crate) fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

pub(crate) fn replace_bytes(haystack: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(haystack.len());
    let mut rest = haystack;
    while let Some(index) = find_bytes(rest, from) {
        result.extend_from_slice(&rest[..index]);
        result.extend_from_slice(to);
        rest = &rest[index + from.len()..];
    }
    result.extend_from_slice(rest);
    result
}
