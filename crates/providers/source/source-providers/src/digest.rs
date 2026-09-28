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

pub(crate) fn path_source_digest(path: &Path, identity_ignore: &[String]) -> String {
    let ignored = identity_ignore
        .iter()
        .map(String::as_str)
        .chain(DEFAULT_PATH_SOURCE_IGNORES.iter().copied())
        .collect::<Vec<_>>();
    tree_digest(path, &ignored)
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
