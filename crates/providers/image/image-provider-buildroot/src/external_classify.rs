//! What a changed `BR2_EXTERNAL` file that the package mapping names no
//! package for affects, so a change to it cleans no more than it must:
//! - a file a package's `.config` setting names (a kernel config fragment,
//!   a package's config file) rebuilds that package (`owning_package`), and
//!   the skeleton of `BR2_ROOTFS_SKELETON_CUSTOM_PATH` rebuilds
//!   `skeleton-custom`;
//! - a file under an overlay, a device or users table, or beside the
//!   post-build or post-fakeroot script is installed by target finalization:
//!   `target/` is reassembled, no package rebuilds;
//! - Kconfig and documentation files (`Config.in*`, `external.desc`,
//!   `configs/*_defconfig`, Markdown, `README*`, `LICENSE*`, `docs/`) have no
//!   effect of their own: a change to them shows up as a `.config` change;
//! - a file beside the post-image script only reruns image operations, which
//!   fingerprint their scripts, so nothing is cleaned;
//! - a `.mk` file may assign anything: a full clean; any other file has no
//!   known effect: target finalization is reassembled.
//!
//! The settings are read from the `.config` the tree is configured with.
use super::*;
use gaia_image_providers::{ExternalTree, expand_external_paths};

/// Settings whose paths a target finalize reads: a file under one is
/// installed (or read) by it.
const FINALIZE_PATH_SETTINGS: &[&str] = &[
    "BR2_ROOTFS_OVERLAY",
    "BR2_ROOTFS_DEVICE_TABLE",
    "BR2_ROOTFS_USERS_TABLES",
    "BR2_ROOTFS_POST_BUILD_SCRIPT",
    "BR2_ROOTFS_POST_FAKEROOT_SCRIPT",
];

/// Scripts whose siblings (regular files beside them) finalize reads.
const FINALIZE_SCRIPT_SETTINGS: &[&str] = &[
    "BR2_ROOTFS_POST_BUILD_SCRIPT",
    "BR2_ROOTFS_POST_FAKEROOT_SCRIPT",
];

const POST_IMAGE_SCRIPT_SETTING: &str = "BR2_ROOTFS_POST_IMAGE_SCRIPT";
const SKELETON_SETTING: &str = "BR2_ROOTFS_SKELETON_CUSTOM_PATH";
const SKELETON_PACKAGE: &str = "skeleton-custom";

/// What the changed external files of a tree affect.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ExternalClassification {
    /// Packages the changed files rebuild (added to the package overrides).
    pub(crate) packages: BTreeSet<String>,
    /// One line per changed file that rebuilds packages or needs no clean.
    pub(crate) reasons: Vec<String>,
    /// One line per changed file that only target finalization reads.
    pub(crate) finalize: Vec<String>,
    /// Changed `.mk` files, keys `<tree>:<path>`: a full clean.
    pub(crate) unmapped: Vec<String>,
}

/// A setting's value: the absolute paths it names, with `$(BR2_EXTERNAL_*)`
/// expanded and normalized (see [`normal_path`]).
struct Setting {
    key: String,
    paths: Vec<PathBuf>,
}

impl Setting {
    /// The setting names `path`: the file itself, or a directory it lies in.
    fn names(&self, path: &Path) -> bool {
        self.paths.iter().any(|named| path.starts_with(named))
    }

    /// The setting names a script whose directory `path` is in.
    fn names_beside(&self, path: &Path) -> bool {
        self.paths
            .iter()
            .any(|named| named.parent().is_some() && named.parent() == path.parent())
    }
}

enum Class {
    Packages {
        packages: BTreeSet<String>,
        read_by: Vec<String>,
    },
    Finalize(String),
    Nothing(String),
    Full,
}

/// A path with its parent directory canonicalized when it exists, so a
/// symlinked checkout compares equal to the path a setting names.
fn normal_path(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => fs::canonicalize(parent)
            .map(|parent| parent.join(name))
            .unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

/// The settings of a `.config` (`KEY=value` lines), with their paths.
fn settings(config: &str, trees: &[ExternalTree]) -> Vec<Setting> {
    config
        .lines()
        .filter_map(|line| {
            let (key, value) = line.trim().split_once('=')?;
            let key = key.trim();
            if !key.starts_with("BR2_") {
                return None;
            }
            let value = expand_external_paths(value.trim().trim_matches('"'), trees);
            let paths = value
                .split_whitespace()
                .map(Path::new)
                .filter(|path| path.is_absolute())
                .map(normal_path)
                .collect();
            Some(Setting {
                key: key.to_string(),
                paths,
            })
        })
        .collect()
}

/// Kconfig, documentation and tree metadata: no effect of its own.
fn is_metadata(relative: &str) -> bool {
    let parts = relative.split('/').collect::<Vec<_>>();
    let name = parts.last().copied().unwrap_or_default();
    name.starts_with("Config.in")
        || name == "external.desc"
        || (parts.first() == Some(&"configs") && name.ends_with("_defconfig"))
        || name.ends_with(".md")
        || name.starts_with("README")
        || name.starts_with("LICENSE")
        || parts.iter().any(|part| *part == "docs" || *part == "doc")
}

fn classify_file(
    key: &str,
    path: &Path,
    relative: &str,
    settings: &[Setting],
    known: &BTreeSet<&str>,
) -> Class {
    // A setting a package reads, or the skeleton: that package rebuilds.
    let mut packages = BTreeSet::new();
    let mut read_by = Vec::new();
    for setting in settings.iter().filter(|setting| setting.names(path)) {
        if setting.key == SKELETON_SETTING {
            packages.insert(SKELETON_PACKAGE.to_string());
            read_by.push(setting.key.clone());
        } else if let Some(package) = owning_package(&setting.key, known) {
            packages.insert(package.to_string());
            read_by.push(setting.key.clone());
        }
    }
    if !packages.is_empty() {
        return Class::Packages { packages, read_by };
    }

    // Read by target finalization: overlays, tables and the post-build and
    // post-fakeroot scripts.
    let finalize_by = settings
        .iter()
        .filter(|setting| {
            FINALIZE_PATH_SETTINGS.contains(&setting.key.as_str()) && setting.names(path)
        })
        .map(|setting| setting.key.clone())
        .collect::<Vec<_>>();
    if !finalize_by.is_empty() {
        return Class::Finalize(format!(
            "buildroot external file {key} changed: read by {}; reassembling target",
            finalize_by.join(", ")
        ));
    }

    if is_metadata(relative) {
        return Class::Nothing(
            "Kconfig or documentation file, its effect shows in .config".to_string(),
        );
    }

    let beside_finalize = settings
        .iter()
        .filter(|setting| {
            FINALIZE_SCRIPT_SETTINGS.contains(&setting.key.as_str()) && setting.names_beside(path)
        })
        .map(|setting| setting.key.clone())
        .collect::<Vec<_>>();
    if !beside_finalize.is_empty() {
        return Class::Finalize(format!(
            "buildroot external file {key} changed: beside {}; reassembling target",
            beside_finalize.join(", ")
        ));
    }

    let beside_post_image = settings.iter().any(|setting| {
        setting.key == POST_IMAGE_SCRIPT_SETTING
            && (setting.names(path) || setting.names_beside(path))
    });
    if beside_post_image {
        return Class::Nothing(
            "beside the post-image script, which reruns on its fingerprint".to_string(),
        );
    }

    if relative.ends_with(".mk") {
        return Class::Full;
    }
    Class::Finalize(format!(
        "buildroot external file {key} changed and maps to no package; reassembling target"
    ))
}

/// The classification of changed external files: `unmapped` are the keys
/// the package mapping named no package for (see `external_changes`).
pub(crate) fn classify_external_changes(
    unmapped: &[String],
    trees: &[ExternalTree],
    config: &str,
    known: &BTreeSet<&str>,
) -> ExternalClassification {
    let settings = settings(config, trees);
    let mut classification = ExternalClassification::default();
    for key in unmapped {
        let (tree_name, relative) = key.split_once(':').unwrap_or(("", key));
        let Some(tree) = trees.iter().find(|tree| tree.name == tree_name) else {
            classification.finalize.push(format!(
                "buildroot external file {key} changed and maps to no package; reassembling target"
            ));
            continue;
        };
        let path = normal_path(&tree.dir.join(relative));
        match classify_file(key, &path, relative, &settings, known) {
            Class::Packages { packages, read_by } => {
                classification.reasons.push(format!(
                    "buildroot external file {key} changed: rebuilds {} (read by {})",
                    packages.iter().cloned().collect::<Vec<_>>().join(", "),
                    read_by.join(", ")
                ));
                classification.packages.extend(packages);
            }
            Class::Finalize(line) => classification.finalize.push(line),
            Class::Nothing(why) => classification.reasons.push(format!(
                "buildroot external file {key} changed: {why}; no clean"
            )),
            Class::Full => classification.unmapped.push(key.clone()),
        }
    }
    classification
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trees() -> Vec<ExternalTree> {
        vec![ExternalTree {
            name: "RAZE".to_string(),
            dir: PathBuf::from("/nonexistent-workspace/raze"),
        }]
    }

    fn known() -> BTreeSet<&'static str> {
        BTreeSet::from(["linux", "zlib", "skeleton-custom"])
    }

    fn classify(keys: &[&str], config: &str) -> ExternalClassification {
        let keys = keys.iter().map(|key| key.to_string()).collect::<Vec<_>>();
        classify_external_changes(&keys, &trees(), config, &known())
    }

    #[test]
    fn a_kernel_fragment_in_the_board_rebuilds_linux() {
        let classification = classify(
            &["RAZE:board/raze/linux.fragment"],
            "BR2_LINUX_KERNEL_CONFIG_FRAGMENT_FILES=\"$(BR2_EXTERNAL_RAZE_PATH)/board/raze/linux.fragment\"\n",
        );
        assert_eq!(
            classification.packages,
            BTreeSet::from(["linux".to_string()])
        );
        assert_eq!(classification.reasons.len(), 1, "{classification:?}");
        assert!(classification.finalize.is_empty());
        assert!(classification.unmapped.is_empty());
    }

    #[test]
    fn a_file_a_package_setting_names_rebuilds_that_package() {
        let classification = classify(
            &["RAZE:board/raze/zlib.conf"],
            "BR2_PACKAGE_ZLIB_CONFIG_FILE=\"/nonexistent-workspace/raze/board/raze/zlib.conf\"\n",
        );
        assert_eq!(
            classification.packages,
            BTreeSet::from(["zlib".to_string()])
        );
    }

    #[test]
    fn an_overlay_file_reassembles_the_target_and_rebuilds_nothing() {
        let classification = classify(
            &["RAZE:board/raze/overlay/etc/motd"],
            "BR2_ROOTFS_OVERLAY=\"$(BR2_EXTERNAL_RAZE_PATH)/board/raze/overlay\"\n",
        );
        assert!(classification.packages.is_empty());
        assert!(classification.unmapped.is_empty());
        assert_eq!(classification.finalize.len(), 1, "{classification:?}");
        assert!(classification.finalize[0].contains("BR2_ROOTFS_OVERLAY"));
    }

    #[test]
    fn a_skeleton_file_rebuilds_the_skeleton_package() {
        let classification = classify(
            &["RAZE:board/raze/skeleton/etc/fstab"],
            "BR2_ROOTFS_SKELETON_CUSTOM_PATH=\"$(BR2_EXTERNAL_RAZE_PATH)/board/raze/skeleton\"\n",
        );
        assert_eq!(
            classification.packages,
            BTreeSet::from(["skeleton-custom".to_string()])
        );
        assert!(classification.finalize.is_empty());
    }

    #[test]
    fn a_post_build_script_sibling_reassembles_the_target() {
        let classification = classify(
            &["RAZE:board/raze/hostname.txt"],
            "BR2_ROOTFS_POST_BUILD_SCRIPT=\"$(BR2_EXTERNAL_RAZE_PATH)/board/raze/post-build.sh\"\n",
        );
        assert!(classification.packages.is_empty());
        assert_eq!(classification.finalize.len(), 1, "{classification:?}");
    }

    #[test]
    fn a_kconfig_or_documentation_file_cleans_nothing() {
        for key in [
            "RAZE:Config.in",
            "RAZE:external.desc",
            "RAZE:configs/raze_defconfig",
            "RAZE:docs/notes.txt",
            "RAZE:README.md",
        ] {
            let classification = classify(&[key], "");
            assert!(
                classification.finalize.is_empty(),
                "{key}: {classification:?}"
            );
            assert!(
                classification.unmapped.is_empty(),
                "{key}: {classification:?}"
            );
            assert_eq!(classification.reasons.len(), 1, "{key}: {classification:?}");
            assert!(classification.reasons[0].ends_with("no clean"), "{key}");
        }
    }

    #[test]
    fn a_post_image_script_sibling_cleans_nothing() {
        let classification = classify(
            &["RAZE:board/raze/layout.txt"],
            "BR2_ROOTFS_POST_IMAGE_SCRIPT=\"$(BR2_EXTERNAL_RAZE_PATH)/board/raze/post-image.sh\"\n",
        );
        assert!(classification.finalize.is_empty());
        assert!(classification.unmapped.is_empty());
        assert_eq!(classification.reasons.len(), 1);
    }

    #[test]
    fn a_changed_mk_file_is_a_full_clean() {
        let classification = classify(&["RAZE:board/raze/hooks.mk"], "");
        assert_eq!(classification.unmapped, ["RAZE:board/raze/hooks.mk"]);
        assert!(classification.finalize.is_empty());
    }

    #[test]
    fn any_other_changed_file_reassembles_the_target() {
        let classification = classify(&["RAZE:board/raze/hook.sh"], "");
        assert!(classification.packages.is_empty());
        assert!(classification.unmapped.is_empty());
        assert_eq!(
            classification.finalize,
            [
                "buildroot external file RAZE:board/raze/hook.sh changed and maps to no package; reassembling target"
            ]
        );
    }
}
