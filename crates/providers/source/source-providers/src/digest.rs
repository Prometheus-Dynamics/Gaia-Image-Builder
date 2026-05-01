use super::*;

pub(crate) fn verify_sha256(path: &Path, expected_sha: &str) -> Result<(), SourceProviderError> {
    let actual = sha256_or_placeholder(path);
    if actual == expected_sha {
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

pub(crate) fn sha256_or_placeholder(path: &Path) -> String {
    let output = Command::new("sha256sum").arg(path).output().ok();
    let Some(output) = output else {
        return format!("sha256-unavailable:{}", path.display());
    };
    if !output.status.success() {
        return format!(
            "sha256-error:{}:{}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn tree_digest(path: &Path, ignored_names: &[&str]) -> String {
    let mut hasher = DefaultHasher::new();
    hash_tree(path, ignored_names, &mut hasher);
    format!("{:016x}", hasher.finish())
}

pub(crate) fn path_source_digest(path: &Path, identity_ignore: &[String]) -> String {
    let ignored = identity_ignore
        .iter()
        .map(String::as_str)
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
