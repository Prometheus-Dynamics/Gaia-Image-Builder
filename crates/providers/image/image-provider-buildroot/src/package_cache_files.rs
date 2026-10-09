//! File-level helpers of the package cache: walking per-package trees,
//! cloning entries in and out, finding the output path in files, and the
//! order packages and their stamps are restored in.
use super::*;

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
