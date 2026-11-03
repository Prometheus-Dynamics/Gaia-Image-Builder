//! The extract cache of verified archives: a `url` archive's extracted tree,
//! keyed by what decides its content (the archive's SHA-256,
//! `strip_components`, `members` and `allow_absolute_symlinks`), under
//! `<workspace>/.gaia/cache/extracted/<key>`. A source that materializes again,
//! or in another build dir, copies the tree instead of decompressing the
//! archive (650 MB of linux-firmware for the seven files a Raze ships).

use super::*;
use sha2::{Digest, Sha256};

/// Workspace-relative root of the extract cache.
pub const EXTRACT_CACHE_DIR: &str = ".gaia/cache/extracted";

/// The cache directory of `archive`'s extracted tree, or `None` for an
/// archive whose content is not fixed by a checksum (a `path` archive).
pub(crate) fn extract_cache_dir(
    workspace_root: &Path,
    archive: &gaia_spec::ArchiveSourceSpec,
) -> Option<PathBuf> {
    archive.url.as_ref()?;
    let sha = archive.sha256.as_deref()?.to_ascii_lowercase();
    if sha.len() != 64 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(format!(
        "extract-cache-v1\nsha256={sha}\nstrip_components={}\nallow_absolute_symlinks={}\n",
        archive.strip_components, archive.allow_absolute_symlinks
    ));
    for member in &archive.members {
        hasher.update(b"member=");
        hasher.update(member.as_bytes());
        hasher.update(b"\0");
    }
    let key: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Some(workspace_root.join(EXTRACT_CACHE_DIR).join(key))
}

/// Copies `from`'s contents into `to` (an existing directory), keeping links,
/// modes and hard links; reflinks where the filesystem has them.
pub(crate) fn copy_tree(from: &Path, to: &Path) -> Result<(), SourceProviderError> {
    let output = Command::new("cp")
        .arg("-a")
        .arg("--reflink=auto")
        .arg(format!("{}/.", from.display()))
        .arg(to)
        .output()
        .map_err(|error| {
            SourceProviderError::new(
                SourceProviderErrorKind::BackendCommand,
                format!("failed to run cp for '{}': {error}", from.display()),
            )
        })?;
    if output.status.success() {
        return Ok(());
    }
    Err(SourceProviderError::new(
        SourceProviderErrorKind::BackendCommand,
        format!(
            "failed to copy '{}' to '{}': {}",
            from.display(),
            to.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    ))
}

/// Stores the extracted tree `extracted` as `cache_dir`, through a temporary
/// directory renamed into place, so a cache entry is always complete. A
/// failure only costs the next materialization an extract.
pub(crate) fn store(extracted: &Path, cache_dir: &Path) {
    if cache_dir.is_dir() {
        return;
    }
    let Some(parent) = cache_dir.parent() else {
        return;
    };
    let temp = parent.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        cache_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("entry")
    ));
    let result = fs::create_dir_all(&temp)
        .map_err(|error| error.to_string())
        .and_then(|()| copy_tree(extracted, &temp).map_err(|error| error.message))
        .and_then(|()| fs::rename(&temp, cache_dir).map_err(|error| error.to_string()));
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&temp);
        tracing::warn!(cache = %cache_dir.display(), %error, "failed to store the extracted archive in the cache");
    }
}
