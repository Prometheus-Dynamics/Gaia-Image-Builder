//! What decides a full Buildroot clean: the settings and package override
//! contents that can change what packages build, and nothing else.
//!
//! A full clean costs a from-scratch rebuild (often over an hour), so changes
//! that only affect filesystem image generation (`BR2_TARGET_ROOTFS_*`,
//! post-image and fakeroot scripts) or where things are downloaded and cached
//! must not trigger one. Buildroot regenerates images (applying the users
//! and device tables) on every `make`, and reruns the post-image script, so
//! editing that script's contents reruns only image generation.
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
    // Users and device tables are applied whenever rootfs images are
    // generated, on every `make`. Their paths often name an import-source
    // checkout, which changes with the source's rev.
    "BR2_ROOTFS_USERS_TABLES",
    "BR2_ROOTFS_DEVICE_TABLE",
    "BR2_ROOTFS_STATIC_DEVICE_TABLE",
];

fn setting_requires_clean(key: &str) -> bool {
    !SETTINGS_NOT_REQUIRING_CLEAN
        .iter()
        .any(|pattern| match pattern.strip_suffix('*') {
            Some(prefix) => key.starts_with(prefix),
            None => key == *pattern,
        })
}

/// `.config` key of a `KEY=value` or `# KEY is not set` line; other comments
/// are headers.
fn setting_key(line: &str) -> Option<&str> {
    match line.strip_prefix("# ") {
        Some(rest) if rest.ends_with(" is not set") => rest.split(' ').next(),
        Some(_) => None,
        None if line.starts_with('#') => None,
        None => line.split('=').next(),
    }
}

/// The checkout directory of an import source is named after its rev
/// (`.gaia/cache/import-sources/<id>-<rev>`), so a `BR2_EXTERNAL_*_PATH` or
/// table path into it changes with every rev bump even when nothing that
/// builds does. What the files there contain is compared elsewhere.
fn without_import_source_revs(line: &str) -> String {
    const MARKER: &str = "import-sources/";
    let mut normalized = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find(MARKER) {
        let after = &rest[start + MARKER.len()..];
        let end = after.find(['/', '"', ' ', ':']).unwrap_or(after.len());
        normalized.push_str(&rest[..start + MARKER.len()]);
        normalized.push_str("<checkout>");
        rest = &after[end..];
    }
    normalized.push_str(rest);
    normalized
}

/// The `.config` settings that can change what packages build, by key.
fn rebuild_settings(config: &str) -> BTreeMap<&str, String> {
    config
        .lines()
        .map(str::trim)
        .filter_map(|line| {
            let key = setting_key(line).filter(|key| setting_requires_clean(key))?;
            Some((key, without_import_source_revs(line)))
        })
        .collect()
}

/// Keys that were added, removed or changed between two configs.
fn changed_rebuild_settings(previous: &str, current: &str) -> Vec<String> {
    let previous = rebuild_settings(previous);
    let current = rebuild_settings(current);
    previous
        .keys()
        .chain(current.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| previous.get(*key) != current.get(*key))
        .map(|key| key.to_string())
        .collect()
}

/// The settings requiring a clean that differ between the current `.config`
/// and the last built one (empty when none do), or `None` when no snapshot
/// exists (trees built by older Gaia versions).
pub(crate) fn config_changes_since_snapshot(output_dir: &Path) -> Option<Vec<String>> {
    let previous = fs::read_to_string(output_dir.join(CONFIG_SNAPSHOT)).ok()?;
    let current = fs::read_to_string(output_dir.join(".config")).ok()?;
    Some(changed_rebuild_settings(&previous, &current))
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
    fn users_and_device_tables_do_not_require_a_clean() {
        let with_tables = |checkout: &str| {
            format!(
                "{BASE}BR2_ROOTFS_USERS_TABLES=\"/work/.gaia/cache/import-sources/{checkout}/users.table\"\n\
                 BR2_ROOTFS_DEVICE_TABLE=\"system/device_table.txt {checkout}/device.table\"\n\
                 BR2_ROOTFS_STATIC_DEVICE_TABLE=\"system/device_table_dev.txt {checkout}/dev.table\"\n"
            )
        };
        assert_eq!(
            rebuild_settings(&with_tables("orion-rev1")),
            rebuild_settings(&with_tables("orion-rev2"))
        );
        assert_eq!(
            rebuild_settings(BASE),
            rebuild_settings(&with_tables("orion-rev1"))
        );
        for key in [
            "BR2_ROOTFS_USERS_TABLES",
            "BR2_ROOTFS_DEVICE_TABLE",
            "BR2_ROOTFS_STATIC_DEVICE_TABLE",
        ] {
            assert!(!setting_requires_clean(key), "{key}");
        }
        assert!(setting_requires_clean("BR2_ROOTFS_OVERLAY"));
    }

    #[test]
    fn package_and_toolchain_settings_require_a_clean() {
        let enabled = BASE.replace("# BR2_PACKAGE_FFMPEG is not set", "BR2_PACKAGE_FFMPEG=y");
        assert_ne!(rebuild_settings(BASE), rebuild_settings(&enabled));
        let arch = BASE.replace("BR2_aarch64=y", "BR2_arm=y");
        assert_ne!(rebuild_settings(BASE), rebuild_settings(&arch));
    }

    #[test]
    fn changed_settings_are_named() {
        let changed = BASE
            .replace("# BR2_PACKAGE_FFMPEG is not set", "BR2_PACKAGE_FFMPEG=y")
            .replace("BR2_PACKAGE_LIBCAMERA=y\n", "")
            .replace("\"1G\"", "\"2G\"")
            + "BR2_PACKAGE_HTOP=y\n";
        assert_eq!(
            changed_rebuild_settings(BASE, &changed),
            [
                "BR2_PACKAGE_FFMPEG",
                "BR2_PACKAGE_HTOP",
                "BR2_PACKAGE_LIBCAMERA"
            ]
        );
        assert!(changed_rebuild_settings(BASE, BASE).is_empty());
    }

    #[test]
    fn import_source_rev_bumps_do_not_require_a_clean() {
        let with_external = |checkout: &str| {
            format!(
                "BR2_EXTERNAL_RAZE_DEVICE_PATH=\"/work/.gaia/cache/import-sources/{checkout}/devices/raze/external\"\n\
                 BR2_GLOBAL_PATCH_DIR=\"board/patches /work/.gaia/cache/import-sources/{checkout}/patches\"\n{BASE}"
            )
        };
        assert!(
            changed_rebuild_settings(
                &with_external("atlas-c881c60fb03e977c42948a4dd898d032f55d3d92"),
                &with_external("atlas-fd52491d21bd8a4a6c783df8ff066cf7624bec06"),
            )
            .is_empty()
        );
        let moved = with_external("atlas-c881c60").replace("/devices/raze/", "/devices/argos/");
        assert_eq!(
            changed_rebuild_settings(&with_external("atlas-c881c60"), &moved),
            ["BR2_EXTERNAL_RAZE_DEVICE_PATH"]
        );
    }

    #[test]
    fn post_image_script_never_requires_a_clean() {
        let with_script =
            |script: &str| format!("{BASE}BR2_ROOTFS_POST_IMAGE_SCRIPT=\"{script}\"\n");
        assert!(
            changed_rebuild_settings(
                &with_script("/work/raze/post-image.sh"),
                &with_script("/work/raze/post-image-v2.sh /work/sign.sh"),
            )
            .is_empty()
        );
        assert!(
            changed_rebuild_settings(BASE, &with_script("/work/raze/post-image.sh")).is_empty()
        );
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
