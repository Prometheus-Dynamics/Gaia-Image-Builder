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

pub(crate) fn setting_requires_clean(key: &str) -> bool {
    !SETTINGS_NOT_REQUIRING_CLEAN
        .iter()
        .any(|pattern| match pattern.strip_suffix('*') {
            Some(prefix) => key.starts_with(prefix),
            None => key == *pattern,
        })
}

/// The key and value of a `KEY=value` line. `# KEY is not set` and
/// `KEY=n` lines, like headers, yield nothing: an unset option and one
/// Kconfig no longer shows build the same.
fn setting(line: &str) -> Option<(&str, &str)> {
    if line.starts_with('#') {
        return None;
    }
    line.split_once('=').filter(|(_, value)| *value != "n")
}

/// Settings that only describe `BR2_EXTERNAL` trees (their names, checkout
/// paths and `git describe` versions). What a tree provides is compared as
/// packages: their options, versions and override contents.
fn describes_external_tree(key: &str) -> bool {
    key.starts_with("BR2_EXTERNAL")
}

/// The checkout directory of an import source is named after its rev
/// (`.gaia/cache/import-sources/<id>-<rev>`), so a table or patch path into
/// it changes with every rev bump even when nothing that builds does. What
/// the files there contain is compared elsewhere.
fn without_import_source_revs(value: &str) -> String {
    const MARKER: &str = "import-sources/";
    let mut normalized = String::with_capacity(value.len());
    let mut rest = value;
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
        .filter_map(setting)
        .filter(|(key, _)| setting_requires_clean(key) && !describes_external_tree(key))
        .map(|(key, value)| (key, without_import_source_revs(value)))
        .collect()
}

/// A setting that was set, unset or changed between two configs. `None` is
/// an unset (or no longer shown) option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfigChange {
    pub key: String,
    pub previous: Option<String>,
    pub current: Option<String>,
}

impl ConfigChange {
    /// A bool option that was off and is now on.
    pub(crate) fn enables(&self) -> bool {
        self.previous.is_none() && self.current.as_deref() == Some("y")
    }
}

fn changed_rebuild_settings(previous: &str, current: &str) -> Vec<ConfigChange> {
    let previous = rebuild_settings(previous);
    let current = rebuild_settings(current);
    previous
        .keys()
        .chain(current.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| previous.get(*key) != current.get(*key))
        .map(|key| ConfigChange {
            key: key.to_string(),
            previous: previous.get(key).cloned(),
            current: current.get(key).cloned(),
        })
        .collect()
}

/// The settings requiring a clean that differ between the current `.config`
/// and the last built one (empty when none do), or `None` when no snapshot
/// exists (trees built by older Gaia versions).
pub(crate) fn config_changes_since_snapshot(output_dir: &Path) -> Option<Vec<ConfigChange>> {
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

/// Per-package content digests of the package override trees, by package
/// directory name, so a change rebuilds only the packages it touches. The
/// first tree providing a package wins, as when they are materialized.
const OVERRIDE_DIGESTS_STATE: &str = ".gaia-buildroot-package-overrides.digests";

pub(crate) fn package_override_digests(dirs: &[PathBuf]) -> BTreeMap<String, String> {
    let mut digests = BTreeMap::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if path.is_dir() && !digests.contains_key(&name) {
                let mut hasher = Sha256::new();
                hash_tree(&path, &path, &mut hasher);
                digests.insert(name, hex(&hasher.finalize()));
            }
        }
    }
    digests
}

/// The previous run's per-package override digests, or `None` for trees
/// recorded by older Gaia versions.
pub(crate) fn read_package_override_digests(output_dir: &Path) -> Option<BTreeMap<String, String>> {
    let state = fs::read_to_string(output_dir.join(OVERRIDE_DIGESTS_STATE)).ok()?;
    Some(
        state
            .lines()
            .filter_map(|line| line.split_once(' '))
            .map(|(name, digest)| (name.to_string(), digest.to_string()))
            .collect(),
    )
}

pub(crate) fn write_package_override_digests(
    output_dir: &Path,
    digests: &BTreeMap<String, String>,
) -> Result<(), ImageProviderError> {
    let state = digests
        .iter()
        .map(|(name, digest)| format!("{name} {digest}\n"))
        .collect::<String>();
    fs::write(output_dir.join(OVERRIDE_DIGESTS_STATE), state).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to record Buildroot package override digests in '{}': {error}",
            output_dir.display()
        ))
    })
}

/// Override packages that were added, removed or changed.
pub(crate) fn changed_override_packages(
    previous: &BTreeMap<String, String>,
    current: &BTreeMap<String, String>,
) -> BTreeSet<String> {
    previous
        .keys()
        .chain(current.keys())
        .filter(|name| previous.get(*name) != current.get(*name))
        .cloned()
        .collect()
}

/// Content digest of a file or directory tree (paths relative to it, modes,
/// contents, symlink targets).
pub(crate) fn path_content_digest(path: &Path) -> String {
    let mut hasher = Sha256::new();
    hash_tree(path, path, &mut hasher);
    hex(&hasher.finalize())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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
    format!("content-v1:{}", hex(&hasher.finalize()))
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
        let changes = changed_rebuild_settings(BASE, &changed);
        assert_eq!(
            changes
                .iter()
                .map(|change| change.key.as_str())
                .collect::<Vec<_>>(),
            [
                "BR2_PACKAGE_FFMPEG",
                "BR2_PACKAGE_HTOP",
                "BR2_PACKAGE_LIBCAMERA"
            ]
        );
        assert!(changes[0].enables() && changes[1].enables());
        assert_eq!(changes[2].previous.as_deref(), Some("y"));
        assert_eq!(changes[2].current, None);
        assert!(changed_rebuild_settings(BASE, BASE).is_empty());
    }

    #[test]
    fn unset_and_hidden_options_are_the_same() {
        let shown = format!("{BASE}# BR2_PACKAGE_PD_IMAGE_SLOTS is not set\n");
        assert!(changed_rebuild_settings(BASE, &shown).is_empty());
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
        let moved = with_external("atlas-c881c60").replace("/patches\"", "/patches-v2\"");
        let changes = changed_rebuild_settings(&with_external("atlas-c881c60"), &moved);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].key, "BR2_GLOBAL_PATCH_DIR");
    }

    #[test]
    fn external_tree_descriptions_are_not_compared() {
        let with_tree = |describe: &str, names: &str| {
            format!(
                "BR2_EXTERNAL_NAMES=\"{names}\"\n\
                 BR2_EXTERNAL_RAZE_DEVICE_PATH=\"/work/atlas/{describe}\"\n\
                 BR2_EXTERNAL_RAZE_DEVICE_VERSION=\"{describe}\"\n\
                 BR2_EXTERNAL_GAIA_GENERATED_VERSION=\"v2024.0.10-{describe}\"\n{BASE}"
            )
        };
        assert!(
            changed_rebuild_settings(
                &with_tree("-g5baf83e", "RAZE_DEVICE"),
                &with_tree("-g696d3ad", "RAZE_DEVICE GAIA_GENERATED"),
            )
            .is_empty()
        );
    }

    #[test]
    fn override_digests_name_the_changed_packages() {
        let base = std::env::temp_dir().join(format!(
            "gaia-override-digests-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let write = |package: &str, contents: &str| {
            fs::create_dir_all(base.join(package)).expect("dir");
            fs::write(base.join(package).join(format!("{package}.mk")), contents).expect("mk");
        };
        write("libcamera", "LIBCAMERA_VERSION = 1\n");
        write("mesa3d", "MESA3D_VERSION = 1\n");
        let before = package_override_digests(std::slice::from_ref(&base));
        write("libcamera", "LIBCAMERA_VERSION = 2\n");
        write("pd-image-slots", "PD_IMAGE_SLOTS_VERSION = 1\n");
        let after = package_override_digests(std::slice::from_ref(&base));
        assert_eq!(
            changed_override_packages(&before, &after),
            BTreeSet::from(["libcamera".to_string(), "pd-image-slots".to_string()])
        );
        write_package_override_digests(&base, &after).expect("record");
        assert_eq!(read_package_override_digests(&base), Some(after));
        let _ = fs::remove_dir_all(base);
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
