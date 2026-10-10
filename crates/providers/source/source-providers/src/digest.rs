use super::*;

/// Compares an already computed digest against the expected one.
pub(crate) fn check_sha256(
    path: &Path,
    expected_sha: &str,
    actual: &str,
) -> Result<(), SourceProviderError> {
    if actual.eq_ignore_ascii_case(expected_sha) {
        return Ok(());
    }
    Err(SourceProviderError::new(
        SourceProviderErrorKind::OutputMissing,
        format!(
            "sha256 mismatch for '{}': expected {}, got {}",
            path.display(),
            expected_sha,
            actual
        ),
    ))
}

/// Hex SHA-256 of a file, hashed in-process (same output as `sha256sum`).
/// Tree digests hash every file, so spawning a process per file was the
/// dominant cost on large source trees.
pub(crate) fn sha256_or_placeholder(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) => return format!("sha256-error:{}:{error}", path.display()),
    };
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => hasher.update(&buffer[..read]),
            Err(error) => return format!("sha256-error:{}:{error}", path.display()),
        }
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Directories that never define a path source's identity: VCS metadata,
/// Gaia's own state, and build or dependency output that builds write into
/// the source tree. Hashing them cost minutes and made every build look like
/// a source change.
pub(crate) const DEFAULT_PATH_SOURCE_IGNORES: &[&str] = &[
    ".git",
    ".gaia",
    "target",
    "node_modules",
    "__pycache__",
    ".gaia-pack",
    ".gaia-wheelhouse",
];

pub(crate) fn tree_digest(path: &Path, ignored_names: &[&str]) -> String {
    let mut hasher = DefaultHasher::new();
    hash_tree(path, ignored_names, &mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Fingerprint of a path source's tree, shared by the planner (reuse) and the
/// provider (materialized state) so both agree on what counts as a change.
///
/// It hashes the set of names and the content of files (plus their mode and
/// length), and the target of symlinks. Directory timestamps and sizes are
/// never hashed, and Gaia's own state is skipped: `.gaia`, `.gaia-trash`,
/// `.gaia-run*` run files, and the workspace build and out directories, even
/// when they sit inside the source root. Creating them after a run therefore
/// does not change the source's fingerprint.
pub fn path_source_fingerprint(
    workspace: &WorkspaceSpec,
    root: &Path,
    identity_ignore: &[String],
) -> String {
    let owned = [&workspace.build_dir, &workspace.out_dir]
        .into_iter()
        .map(|value| {
            let resolved = gaia_spec::resolve_workspace_path(workspace, value)
                .unwrap_or_else(|_| PathBuf::from(&workspace.root_dir).join(value));
            normalize_existing_path(&resolved)
        })
        .collect::<Vec<_>>();
    let ignores = PathSourceIgnores {
        names: identity_ignore
            .iter()
            .cloned()
            .chain(
                DEFAULT_PATH_SOURCE_IGNORES
                    .iter()
                    .map(|name| name.to_string()),
            )
            .collect(),
        owned,
    };
    let root = normalize_existing_path(root);
    let mut hasher = DefaultHasher::new();
    root.display().to_string().hash(&mut hasher);
    hash_path_source_entry(&root, true, &ignores, &mut hasher);
    format!("{:016x}", hasher.finish())
}

struct PathSourceIgnores {
    names: Vec<String>,
    owned: Vec<PathBuf>,
}

impl PathSourceIgnores {
    fn skips(&self, path: &Path) -> bool {
        if self.owned.iter().any(|owned| owned == path) {
            return true;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        self.names.iter().any(|ignored| ignored == name) || is_gaia_owned_name(name)
    }
}

fn is_gaia_owned_name(name: &str) -> bool {
    name == ".gaia"
        || name == ".gaia-trash"
        || name == ".gaia-run-interrupted"
        || name.starts_with(".gaia-run.")
}

fn hash_path_source_entry(
    path: &Path,
    is_root: bool,
    ignores: &PathSourceIgnores,
    hasher: &mut DefaultHasher,
) {
    if !is_root {
        if ignores.skips(path) {
            return;
        }
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .hash(hasher);
    }
    // The root may be a symlink to the tree, so it is followed; entries are not.
    let metadata = match if is_root {
        fs::metadata(path)
    } else {
        fs::symlink_metadata(path)
    } {
        Ok(metadata) => metadata,
        Err(_) => {
            "missing".hash(hasher);
            return;
        }
    };
    if metadata.file_type().is_symlink() {
        "symlink".hash(hasher);
        fs::read_link(path)
            .map(|target| target.display().to_string())
            .unwrap_or_default()
            .hash(hasher);
    } else if metadata.is_dir() {
        "dir".hash(hasher);
        match fs::read_dir(path) {
            Ok(entries) => {
                let mut entries = entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .collect::<Vec<_>>();
                entries.sort();
                for entry in entries {
                    hash_path_source_entry(&entry, false, ignores, hasher);
                }
            }
            Err(_) => "unreadable".hash(hasher),
        }
    } else if metadata.is_file() {
        "file".hash(hasher);
        metadata.len().hash(hasher);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode().hash(hasher);
        }
        hash_file_contents(path, hasher);
    } else {
        "other".hash(hasher);
    }
}

fn hash_file_contents(path: &Path, hasher: &mut DefaultHasher) {
    use std::io::Read;

    let Ok(mut file) = fs::File::open(path) else {
        "unreadable".hash(hasher);
        return;
    };
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => hasher.write(&buffer[..read]),
            Err(_) => {
                "unreadable".hash(hasher);
                return;
            }
        }
    }
}

/// Canonicalizes the longest existing ancestor of `path` and re-appends the
/// rest, so a path that does not exist yet compares equal to its later form.
fn normalize_existing_path(path: &Path) -> PathBuf {
    let mut missing = Vec::new();
    let mut current = path;
    loop {
        if let Ok(canonical) = fs::canonicalize(current) {
            let mut normalized = canonical;
            for part in missing.iter().rev() {
                normalized.push(part);
            }
            return normalized;
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name.to_os_string());
                current = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

pub(crate) fn hash_tree(path: &Path, ignored_names: &[&str], hasher: &mut DefaultHasher) {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if ignored_names.iter().any(|ignored| ignored == &file_name) {
        return;
    }
    path.display().to_string().hash(hasher);
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => {
            "missing".hash(hasher);
            return;
        }
    };
    metadata.is_dir().hash(hasher);
    metadata.is_file().hash(hasher);
    metadata.file_type().is_symlink().hash(hasher);
    metadata.len().hash(hasher);
    if metadata.is_dir() {
        let mut entries = match fs::read_dir(path) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect::<Vec<_>>(),
            Err(_) => return,
        };
        entries.sort();
        for entry in entries {
            hash_tree(&entry, ignored_names, hasher);
        }
    } else if metadata.is_file() {
        sha256_or_placeholder(path).hash(hasher);
    }
}
