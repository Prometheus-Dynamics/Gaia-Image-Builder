//! What decides a full Buildroot clean: the settings and package override
//! contents that can change what packages build, and nothing else.
//!
//! A full clean costs a from-scratch rebuild (often over an hour), so changes
//! that only affect filesystem image generation (`BR2_TARGET_ROOTFS_*`,
//! post-image and fakeroot scripts) or where things are downloaded and cached
//! must not trigger one. Buildroot regenerates images on every `make`.
use super::*;
use sha2::{Digest, Sha256};

/// Snapshot of the `.config` the output tree was last built from. Comparing
/// filtered snapshots, rather than stored digests, lets the filter evolve
/// without forcing a clean of every existing tree.
const CONFIG_SNAPSHOT: &str = ".gaia-buildroot-config.last";

/// Settings (exact names or `prefix*`) that never require rebuilding packages.
const SETTINGS_NOT_REQUIRING_CLEAN: &[&str] = &[
    "BR2_DL_DIR",
    "BR2_CCACHE_DIR",
    "BR2_CCACHE_INITIAL_SETUP",
    "BR2_JLEVEL",
    "BR2_PRIMARY_SITE",
    "BR2_BACKUP_SITE",
    "BR2_KERNEL_MIRROR",
    "BR2_GNU_MIRROR",
    "BR2_LUAROCKS_MIRROR",
    "BR2_CPAN_MIRROR",
    // Filesystem image formats, sizes, compression and labels.
    "BR2_TARGET_ROOTFS_*",
    "BR2_ROOTFS_POST_IMAGE_SCRIPT",
    "BR2_ROOTFS_POST_FAKEROOT_SCRIPT",
];

fn setting_requires_clean(key: &str) -> bool {
    !SETTINGS_NOT_REQUIRING_CLEAN
        .iter()
        .any(|pattern| match pattern.strip_suffix('*') {
            Some(prefix) => key.starts_with(prefix),
            None => key == *pattern,
        })
}

/// The `.config` lines that can change what packages build, in file order.
fn rebuild_settings(config: &str) -> Vec<&str> {
    config
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| {
            // `KEY=value` or `# KEY is not set`; other comments are headers.
            let key = match line.strip_prefix("# ") {
                Some(rest) if rest.ends_with(" is not set") => rest.split(' ').next(),
                Some(_) => None,
                None if line.starts_with('#') => None,
                None => line.split('=').next(),
            };
            key.is_some_and(setting_requires_clean)
        })
        .collect()
}

/// `Some(true)` when the current `.config` differs from the last built one in
/// a setting that requires a clean, `Some(false)` when it does not, and
/// `None` when no snapshot exists (trees built by older Gaia versions).
pub(crate) fn config_requires_clean_since_snapshot(output_dir: &Path) -> Option<bool> {
    let previous = fs::read_to_string(output_dir.join(CONFIG_SNAPSHOT)).ok()?;
    let current = fs::read_to_string(output_dir.join(".config")).ok()?;
    Some(rebuild_settings(&previous) != rebuild_settings(&current))
}

pub(crate) fn write_config_snapshot(output_dir: &Path) -> Result<(), ImageProviderError> {
    let config = output_dir.join(".config");
    if !config.is_file() {
        return Ok(());
    }
    fs::copy(&config, output_dir.join(CONFIG_SNAPSHOT))
        .map(drop)
        .map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to snapshot Buildroot config in '{}': {error}",
                output_dir.display()
            ))
        })
}

/// Digest of the package override trees by content: relative paths, file
/// modes, file contents and symlink targets. Unlike timestamps or absolute
/// paths, it does not change when the trees are re-synced, re-checked out,
/// or read from an import-source checkout whose directory name holds the rev.
pub(crate) fn package_override_content_digest(dirs: &[PathBuf]) -> String {
    let mut hasher = Sha256::new();
    for dir in dirs {
        hasher.update(b"\0dir\0");
        hash_tree(dir, dir, &mut hasher);
    }
    let hex = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("content-v1:{hex}")
}

fn hash_tree(root: &Path, path: &Path, hasher: &mut Sha256) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    let relative = path.strip_prefix(root).unwrap_or(path);
    hasher.update(relative.to_string_lossy().as_bytes());
    hasher.update([0]);
    #[cfg(unix)]
    hasher.update(metadata.permissions().mode().to_le_bytes());
    if metadata.file_type().is_symlink() {
        if let Ok(target) = fs::read_link(path) {
            hasher.update(b"link:");
            hasher.update(target.to_string_lossy().as_bytes());
        }
    } else if metadata.is_file() {
        hasher.update(b"file:");
        hasher.update(file_sha256_or_placeholder(path).as_bytes());
    } else if metadata.is_dir() {
        hasher.update(b"dir");
        let mut entries = fs::read_dir(path)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        entries.sort();
        for entry in entries {
            hash_tree(root, &entry, hasher);
        }
    }
    hasher.update([0]);
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "#\n# Buildroot 2025.02 Configuration\n#\nBR2_aarch64=y\n\
        BR2_PACKAGE_LIBCAMERA=y\n# BR2_PACKAGE_FFMPEG is not set\n\
        BR2_TARGET_ROOTFS_EXT2=y\nBR2_TARGET_ROOTFS_EXT2_SIZE=\"1G\"\n";

    #[test]
    fn rootfs_image_settings_do_not_require_a_clean() {
        let resized = BASE
            .replace("\"1G\"", "\"600M\"")
            .replace("2025.02 Configuration", "2025.02-5-gabc Configuration")
            + "BR2_TARGET_ROOTFS_SQUASHFS=y\nBR2_ROOTFS_POST_IMAGE_SCRIPT=\"board/post-image.sh\"\n";
        assert_eq!(rebuild_settings(BASE), rebuild_settings(&resized));
    }

    #[test]
    fn package_and_toolchain_settings_require_a_clean() {
        let enabled = BASE.replace("# BR2_PACKAGE_FFMPEG is not set", "BR2_PACKAGE_FFMPEG=y");
        assert_ne!(rebuild_settings(BASE), rebuild_settings(&enabled));
        let arch = BASE.replace("BR2_aarch64=y", "BR2_arm=y");
        assert_ne!(rebuild_settings(BASE), rebuild_settings(&arch));
    }

    #[test]
    fn package_override_digest_ignores_location_and_timestamps() {
        let base = std::env::temp_dir().join(format!(
            "gaia-override-digest-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let write = |root: &Path, contents: &str| {
            fs::create_dir_all(root.join("libcamera")).expect("dir");
            fs::write(root.join("libcamera/libcamera.mk"), contents).expect("mk");
        };
        let first = base.join("atlas-rev1/packages");
        let second = base.join("atlas-rev2/packages");
        write(&first, "LIBCAMERA_VERSION = 1\n");
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&second, "LIBCAMERA_VERSION = 1\n");
        assert_eq!(
            package_override_content_digest(std::slice::from_ref(&first)),
            package_override_content_digest(std::slice::from_ref(&second))
        );
        write(&second, "LIBCAMERA_VERSION = 2\n");
        assert_ne!(
            package_override_content_digest(std::slice::from_ref(&first)),
            package_override_content_digest(std::slice::from_ref(&second))
        );
        let _ = fs::remove_dir_all(base);
    }
}
