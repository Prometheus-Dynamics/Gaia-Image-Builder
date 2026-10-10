//! The files of the Buildroot `BR2_EXTERNAL` trees a build uses, by content,
//! and the packages each file touches.
//!
//! An external tree changes what packages build: a `package/<name>/`
//! definition, or a `.mk` file that assigns a package variable such as
//! `HOST_EROFS_UTILS_CONF_OPTS += ...`. The digests show which files changed
//! since the last build; the mapping names the packages to rebuild and the
//! package cache entries that no longer apply. A changed file that maps to no
//! package has no known effect, so the caller falls back to a full clean.
//!
//! Plans and runs both use this module, so `gaia preview` agrees with a run.

use crate::sha256_hex;
use gaia_spec::WorkspaceSpec;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Variable suffixes whose assignment names the package they belong to:
/// `<PKG>_<SUFFIX> =` and `HOST_<PKG>_<SUFFIX> =`.
const PACKAGE_VARIABLE_SUFFIXES: &[&str] = &[
    "CONF_OPTS",
    "CONF_ENV",
    "MAKE_OPTS",
    "MAKE_ENV",
    "DEPENDENCIES",
    "VERSION",
    "SITE",
    "SOURCE",
    "PATCH",
];

/// A `BR2_EXTERNAL` tree: its name (`external.desc`'s `name:`, which names
/// `BR2_EXTERNAL_<NAME>_PATH`) and its directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalTree {
    pub name: String,
    pub dir: PathBuf,
}

/// A file of an external tree: its content digest and the packages it
/// touches (by its `package/<name>/` location or its `.mk` assignments).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExternalFile {
    pub digest: String,
    pub packages: BTreeSet<String>,
}

/// Resolves a path from a build spec: `@alias` paths through the workspace,
/// absolute paths as they are, and relative paths from the workspace root.
pub fn resolve_tree_path(workspace: &WorkspaceSpec, raw: &str) -> PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    if raw.starts_with('@')
        && let Ok(resolved) = gaia_spec::resolve_workspace_path(workspace, raw)
    {
        return resolved;
    }
    Path::new(&workspace.root_dir).join(path)
}

/// The trees of a Buildroot `external_tree` value (colon-separated), in order.
pub fn external_trees(workspace: &WorkspaceSpec, external_tree: Option<&str>) -> Vec<ExternalTree> {
    let Some(external_tree) = external_tree else {
        return Vec::new();
    };
    external_tree
        .split(':')
        .map(str::trim)
        .filter(|tree| !tree.is_empty())
        .map(|raw| {
            let dir = resolve_tree_path(workspace, raw);
            ExternalTree {
                name: tree_name(&dir),
                dir,
            }
        })
        .collect()
}

fn tree_name(dir: &Path) -> String {
    fs::read_to_string(dir.join("external.desc"))
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                line.strip_prefix("name:")
                    .map(|name| name.trim().to_string())
            })
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| {
            dir.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("external")
                .to_string()
        })
}

/// Every file of every tree, keyed `<tree name>:<path in tree>`, recursively,
/// skipping `.git`. A symlink is described by its target, not followed.
pub fn external_tree_files(trees: &[ExternalTree]) -> BTreeMap<String, ExternalFile> {
    let mut files = BTreeMap::new();
    for tree in trees {
        let mut found = Vec::new();
        collect_files(&tree.dir, &tree.dir, &mut found);
        for (relative, path) in found {
            let key = format!("{}:{relative}", tree.name);
            files
                .entry(key)
                .or_insert_with(|| describe_file(&path, &relative));
        }
    }
    files
}

fn collect_files(root: &Path, dir: &Path, found: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        if path.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        let is_real_dir = fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir());
        if is_real_dir {
            collect_files(root, &path, found);
        } else {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            found.push((relative, path));
        }
    }
}

fn describe_file(path: &Path, relative: &str) -> ExternalFile {
    let is_symlink = fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink());
    let mut packages = package_of_path(relative);
    // A `packages/<name>/` override tree: its own package is covered by the
    // package override digests, so only the packages it assigns beyond that
    // one count here.
    let override_owner = relative
        .strip_prefix("packages/")
        .and_then(|rest| rest.split('/').next())
        .map(str::to_string);
    let digest = if is_symlink {
        let target = fs::read_link(path)
            .map(|target| target.display().to_string())
            .unwrap_or_default();
        format!("link:{target}")
    } else if relative.ends_with(".mk") {
        match fs::read(path) {
            Ok(bytes) => {
                packages.extend(packages_assigned_by(&String::from_utf8_lossy(&bytes)));
                let mut hasher = Sha256::new();
                hasher.update(&bytes);
                format!("sha256:{}", hex(&hasher.finalize()))
            }
            Err(error) => format!("unreadable:{error}"),
        }
    } else {
        match sha256_hex(path) {
            Ok(digest) => format!("sha256:{digest}"),
            Err(error) => format!("unreadable:{error}"),
        }
    };
    if let Some(owner) = override_owner {
        packages.remove(&owner);
    }
    ExternalFile { digest, packages }
}

/// The package a path belongs to. A `package/<name>/...` path is Buildroot's
/// layout for an external tree's packages. A `<dir>/patches/<name>/...` path
/// is a patch for package `<name>` (`BR2_GLOBAL_PATCH_DIR` style, at any
/// depth). Patches are mapped here, not only when the package is in the
/// graph, so their package cache keys change too; a name that is not a
/// package is ignored by the planner. A `packages/<name>/` override tree is
/// not counted here; the package override digests already cover it, and a
/// changed override is rebuilt only when its digest changed.
fn package_of_path(relative: &str) -> BTreeSet<String> {
    let parts = relative.split('/').collect::<Vec<_>>();
    let mut packages = BTreeSet::new();
    if parts.len() >= 3 && parts[0] == "package" {
        packages.insert(parts[1].to_string());
    }
    // Buildroot's `linux/` directory of an external tree holds the kernel
    // extensions (`linux-ext-*.mk`, `Config.ext.in`, the kernel's config
    // fragments and patches): all of it belongs to the linux package.
    if parts.len() >= 2 && parts[0] == "linux" {
        packages.insert("linux".to_string());
    }
    // `<name>/patches/<file>` at the top: the patches of package `<name>`.
    if parts.len() >= 3 && parts[1] == "patches" {
        packages.insert(parts[0].to_string());
    }
    // `<name>/<file>` below a `patches` directory: the file is at least two
    // components past it.
    for (index, part) in parts.iter().enumerate() {
        if *part == "patches" && index + 2 < parts.len() {
            packages.insert(parts[index + 1].to_string());
        }
    }
    packages
}

/// The Buildroot packages a makefile's assignments name, by the rule
/// `^\s*(HOST_)?([A-Z0-9_]+?)_(SUFFIX)\s*[+:?]?=`: `HOST_EROFS_UTILS_CONF_OPTS
/// +=` names `host-erofs-utils`. Each package name is lowercase with `-`.
pub fn packages_assigned_by(text: &str) -> BTreeSet<String> {
    let mut packages = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim_start();
        if line.starts_with('#') {
            continue;
        }
        let name_length = line
            .bytes()
            .take_while(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_')
            .count();
        let variable = &line[..name_length];
        let rest = line[name_length..].trim_start_matches([' ', '\t']);
        let rest = rest.strip_prefix(['+', ':', '?']).unwrap_or(rest);
        if !rest.starts_with('=') {
            continue;
        }
        if let Some(package) = package_of_variable(variable) {
            packages.insert(package);
        }
    }
    packages
}

/// The package a package variable (without operator) belongs to.
pub fn package_of_variable(variable: &str) -> Option<String> {
    if let Some(rest) = variable.strip_prefix("HOST_")
        && let Some(name) = package_of_suffixed(rest)
    {
        return Some(format!("host-{}", package_name(name)));
    }
    package_of_suffixed(variable).map(package_name)
}

/// The shortest name before `_` such that the rest is a package variable
/// suffix: a listed suffix, `INSTALL_<...>`, or `<...>_HOOKS`.
fn package_of_suffixed(variable: &str) -> Option<&str> {
    variable.match_indices('_').find_map(|(index, _)| {
        let name = &variable[..index];
        let rest = &variable[index + 1..];
        let names_package = !name.is_empty()
            && (PACKAGE_VARIABLE_SUFFIXES.contains(&rest)
                || rest
                    .strip_prefix("INSTALL_")
                    .is_some_and(|tail| !tail.is_empty())
                || rest
                    .strip_suffix("_HOOKS")
                    .is_some_and(|prefix| !prefix.is_empty()));
        names_package.then_some(name)
    })
}

fn package_name(variable_name: &str) -> String {
    variable_name.to_ascii_lowercase().replace('_', "-")
}

/// Per-package digests of the external files that touch each package, for
/// the package cache keys: a package whose external files change gets a new
/// key, and so does everything built on it.
pub fn external_package_digests(
    files: &BTreeMap<String, ExternalFile>,
) -> BTreeMap<String, String> {
    let mut hashers = BTreeMap::<String, Sha256>::new();
    for (key, file) in files {
        for package in &file.packages {
            let hasher = hashers.entry(package.clone()).or_default();
            hasher.update(key.as_bytes());
            hasher.update([0]);
            hasher.update(file.digest.as_bytes());
            hasher.update([0]);
        }
    }
    hashers
        .into_iter()
        .map(|(package, hasher)| (package, hex(&hasher.finalize())))
        .collect()
}

/// The external files that changed since the recorded state, and what that
/// means for the packages.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ExternalChanges {
    /// Packages to rebuild: those the changed files touch, before or now.
    pub packages: BTreeSet<String>,
    /// Changed files that touch no package. The caller must not guess what
    /// they affect.
    pub unmapped: Vec<String>,
    /// One line per changed file that maps to packages, for run messages.
    pub reasons: Vec<String>,
}

/// Compares the current files with the recorded ones.
pub fn external_changes(
    previous: &BTreeMap<String, ExternalFile>,
    current: &BTreeMap<String, ExternalFile>,
) -> ExternalChanges {
    let mut changes = ExternalChanges::default();
    let keys = previous
        .keys()
        .chain(current.keys())
        .collect::<BTreeSet<_>>();
    for key in keys {
        let before = previous.get(key);
        let after = current.get(key);
        if before.map(|file| &file.digest) == after.map(|file| &file.digest) {
            continue;
        }
        let mut packages = before.map(|file| file.packages.clone()).unwrap_or_default();
        if let Some(after) = after {
            packages.extend(after.packages.iter().cloned());
        }
        if packages.is_empty() {
            changes.unmapped.push(key.clone());
            continue;
        }
        changes.reasons.push(format!(
            "buildroot external file {key} changed: rebuilds {}",
            packages.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
        changes.packages.extend(packages);
    }
    changes
}

/// The recorded state: one `key<TAB>digest<TAB>packages` line per file.
pub fn encode_external_state(files: &BTreeMap<String, ExternalFile>) -> String {
    files
        .iter()
        .map(|(key, file)| {
            format!(
                "{key}\t{}\t{}\n",
                file.digest,
                file.packages.iter().cloned().collect::<Vec<_>>().join(",")
            )
        })
        .collect()
}

pub fn decode_external_state(text: &str) -> BTreeMap<String, ExternalFile> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let key = parts.next()?;
            let digest = parts.next()?;
            let packages = parts
                .next()
                .unwrap_or_default()
                .split(',')
                .filter(|package| !package.is_empty())
                .map(str::to_string)
                .collect();
            Some((
                key.to_string(),
                ExternalFile {
                    digest: digest.to_string(),
                    packages,
                },
            ))
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_external_assignment_names_the_host_package() {
        let packages = packages_assigned_by(
            "HOST_EROFS_UTILS_CONF_OPTS += --enable-multithreading\n\
             LIBCAMERA_DEPENDENCIES = host-meson\n\
             # FOO_VERSION = 1\n\
             PD_IMAGE_SLOTS_SITE := $(BR2_EXTERNAL_RAZE_PATH)/x\n\
             FOO_POST_INSTALL_HOOKS += bar\n\
             FOO_INSTALL_TARGET_CMDS = true\n\
             FOO_SITE_METHOD = git\n",
        );
        assert_eq!(
            packages.into_iter().collect::<Vec<_>>(),
            ["foo", "host-erofs-utils", "libcamera", "pd-image-slots"]
        );
        assert_eq!(
            package_of_variable("HOST_EROFS_UTILS_CONF_OPTS"),
            Some("host-erofs-utils".to_string())
        );
        assert_eq!(package_of_variable("BR2_PACKAGE_FOO"), None);
    }

    #[test]
    fn patches_name_their_package_at_any_depth() {
        assert_eq!(
            package_of_path("board/raze/patches/linux/0001-fix.patch"),
            BTreeSet::from(["linux".to_string()])
        );
        assert_eq!(
            package_of_path("patches/host-erofs-utils/0001.patch"),
            BTreeSet::from(["host-erofs-utils".to_string()])
        );
        // A `patches` directory with no file below a package names none.
        assert!(package_of_path("board/raze/patches/linux").is_empty());
    }

    #[test]
    fn linux_extensions_and_top_level_package_patches_name_their_package() {
        // The raze tree's kernel patch: a linux post-patch hook in external.mk
        // applies linux/patches/*.patch, and no BR2_ setting names it.
        assert_eq!(
            package_of_path(
                "linux/patches/0001-misc-ws2812-pio-rp1-clear_on_probe-parameter.patch"
            ),
            BTreeSet::from(["linux".to_string()])
        );
        assert_eq!(
            package_of_path("linux/ov9782/0001-media-i2c-ov9282-add-ov9782-variant-draft.patch"),
            BTreeSet::from(["linux".to_string()])
        );
        assert_eq!(
            package_of_path("linux/raze.config"),
            BTreeSet::from(["linux".to_string()])
        );
        assert_eq!(
            package_of_path("libcamera/patches/0001-fix.patch"),
            BTreeSet::from(["libcamera".to_string()])
        );
        // A patch directory below a board names no package.
        assert!(package_of_path("board/raze/patches/0001-fix.patch").is_empty());
    }

    #[test]
    fn package_directories_name_their_package() {
        assert_eq!(
            package_of_path("package/host-erofs-utils/host-erofs-utils.mk"),
            BTreeSet::from(["host-erofs-utils".to_string()])
        );
        assert!(package_of_path("Config.in").is_empty());
        assert!(package_of_path("board/raze/post-image.sh").is_empty());
    }

    fn tree_with(files: &[(&str, &str)]) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("gaia-external-files-{nonce}"));
        for (path, contents) in files {
            let full = root.join(path);
            fs::create_dir_all(full.parent().expect("parent")).expect("dir");
            fs::write(full, contents).expect("file");
        }
        root
    }

    #[test]
    fn external_files_are_keyed_by_tree_and_skip_git() {
        let root = tree_with(&[
            ("external.desc", "name: RAZE_DEVICE\n"),
            ("external.mk", "HOST_EROFS_UTILS_CONF_OPTS += -x\n"),
            (".git/HEAD", "ref"),
        ]);
        let trees = vec![ExternalTree {
            name: "RAZE_DEVICE".into(),
            dir: root.clone(),
        }];
        let files = external_tree_files(&trees);
        assert_eq!(
            files.keys().cloned().collect::<Vec<_>>(),
            ["RAZE_DEVICE:external.desc", "RAZE_DEVICE:external.mk"]
        );
        assert_eq!(
            files["RAZE_DEVICE:external.mk"].packages,
            BTreeSet::from(["host-erofs-utils".to_string()])
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_changed_external_mk_names_its_packages_and_unmapped_files_are_reported() {
        let mut before = BTreeMap::new();
        before.insert(
            "RAZE:external.mk".to_string(),
            ExternalFile {
                digest: "sha256:old".into(),
                packages: BTreeSet::from(["host-erofs-utils".to_string()]),
            },
        );
        before.insert(
            "RAZE:Config.in".to_string(),
            ExternalFile {
                digest: "sha256:c".into(),
                packages: BTreeSet::new(),
            },
        );
        let mut after = before.clone();
        after.get_mut("RAZE:external.mk").expect("mk").digest = "sha256:new".into();
        after.get_mut("RAZE:Config.in").expect("cfg").digest = "sha256:c2".into();
        let changes = external_changes(&before, &after);
        assert_eq!(
            changes.packages,
            BTreeSet::from(["host-erofs-utils".to_string()])
        );
        assert_eq!(changes.unmapped, ["RAZE:Config.in"]);
        assert_eq!(changes.reasons.len(), 1);
        assert!(external_changes(&before, &before).packages.is_empty());
    }

    #[test]
    fn external_state_round_trips_and_keys_packages() {
        let mut files = BTreeMap::new();
        files.insert(
            "RAZE:external.mk".to_string(),
            ExternalFile {
                digest: "sha256:a".into(),
                packages: BTreeSet::from(["host-erofs-utils".to_string()]),
            },
        );
        assert_eq!(decode_external_state(&encode_external_state(&files)), files);
        let digests = external_package_digests(&files);
        assert_eq!(
            digests.keys().cloned().collect::<Vec<_>>(),
            ["host-erofs-utils"]
        );
        files.get_mut("RAZE:external.mk").expect("mk").digest = "sha256:b".into();
        assert_ne!(digests, external_package_digests(&files));
    }
}
