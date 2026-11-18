//! Dev reuse (`gaia build --dev-reuse`): a Buildroot output directory kept
//! between builds, so a config change rebuilds the packages it touches and
//! not the whole step.
//!
//! **For bench iteration only.** A clean build is what every other build is,
//! and what a release is: it starts from nothing. A dev build starts from
//! the last dev build of the same identity, and trusts the fingerprint below
//! to know what changed. `gaia build --dev-reuse --verify` builds clean
//! beside it and compares; run it before trusting a dev image for anything
//! but the bench.
//!
//! - **Where:** one warm directory for each identity, under the host's dev
//!   root (`<store>/dev-buildroot/<identity>/out`), outside the content
//!   store, held under an exclusive lock while a build uses it. A second
//!   build of the same identity is refused, not queued.
//! - **Identity:** the step's key without the config text: the step and its
//!   key version, the Buildroot source and Gaia's patches, the host tools'
//!   seed, the make program, the toolchain defaults, the names of the
//!   fragments and of the input trees, the overrides' packages and the
//!   externals. The kernel's, the bootloader's and the root's parts name
//!   different fragments, so each has its own directory.
//! - **Fingerprint** (the manifest, written after a build succeeds): for
//!   each package Buildroot will build (`make show-info`), its own resolved
//!   config lines (`BR2_PACKAGE_<NAME>…`, `BR2_LINUX_KERNEL…` for `linux`,
//!   `BR2_TARGET_<NAME>…` for a bootloader), its version, its downloads and
//!   its dependencies, and the content of every file or directory one of its
//!   lines names. For `linux` and `uboot` the content of their Kconfig files
//!   (the fragment files and a complete config) is kept apart. And the
//!   global lines: every other line of the resolved config, with the content
//!   of what they name.
//! - **What a change does:** a global line changed, a package gone, no
//!   manifest (a first build, or one that failed): the directory is emptied
//!   and the build is clean. A package new or changed: it and everything
//!   that depends on it, in turn, are removed (`<package>-dirclean`) and
//!   built again. A change to only the Kconfig files of `linux` or `uboot`:
//!   its config is made again and Kbuild rebuilds what that config changes
//!   (its own incremental build); what depends on it is removed and built
//!   again. `target/` and `images/` are always removed and made again: with
//!   per-package directories, Buildroot assembles the target from every
//!   package's own. Without per-package directories nothing is reused.
//! - **Harmless lines:** the image formats (`BR2_TARGET_ROOTFS_…`) and the
//!   root's scripts, overlays, users and device tables (`BR2_ROOTFS_…`) are
//!   used only when the target and the images are made, which every build
//!   does again, so they change nothing that is kept.
//!
//! Paths in the config name the store's places, which change with each
//! input tree's content: each is written as its tree's name before it is
//! compared, and the content it names is compared instead.
use gaia_step::Digest;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The manifest's file in a warm directory, beside `out`.
const MANIFEST: &str = "manifest.json";
/// The lock's file in a warm directory.
const LOCK: &str = "lock";
/// What a dev build does, as a number in its step key and its warm
/// directory's identity. Raised when that changes, as `KEY_VERSION` is for a
/// clean build, so a dev result or a warm directory from before is not
/// taken; a clean build's key does not carry it.
pub(crate) const VERSION: u32 = 1;
/// The environment variable that makes a Kconfig-only change of `linux` or
/// `uboot` remove the package instead of configuring it again. For
/// measuring the two against each other; unset in use.
pub(crate) const KCONFIG_ENV: &str = "GAIA_DEV_REUSE_KCONFIG";
/// The packages whose Kconfig files are kept apart, with their options.
const KCONFIG: [(&str, &[&str]); 2] = [
    (
        "linux",
        &[
            "BR2_LINUX_KERNEL_CONFIG_FRAGMENT_FILES",
            "BR2_LINUX_KERNEL_CUSTOM_CONFIG_FILE",
        ],
    ),
    (
        "uboot",
        &[
            "BR2_TARGET_UBOOT_CONFIG_FRAGMENT_FILES",
            "BR2_TARGET_UBOOT_CUSTOM_CONFIG_FILE",
        ],
    ),
];
/// The prefixes of the lines only the target's and the images' making read.
const HARMLESS: [&str; 2] = ["BR2_TARGET_ROOTFS_", "BR2_ROOTFS_"];

/// One package's fingerprint.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Package {
    /// Its config lines, version, downloads and dependencies, and what its
    /// lines name, apart from its Kconfig files.
    pub rest: String,
    /// The content of its Kconfig files (`linux` and `uboot`), else empty.
    pub kconfig: String,
}

/// What a build was made from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Manifest {
    /// The lines no package owns, as written with the trees' names, and what
    /// they name.
    pub global: Vec<String>,
    /// Each package's fingerprint, by name.
    pub packages: BTreeMap<String, Package>,
}

/// What to do with a warm directory before the make.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Reuse {
    /// Empty it and build clean, for this reason.
    Clean(String),
    /// Keep it: remove these packages, and configure these again.
    Keep {
        dirclean: Vec<String>,
        reconfigure: Vec<String>,
    },
}

/// A warm directory, locked while this is held.
pub(crate) struct Warm {
    dir: PathBuf,
    _lock: fs::File,
}

impl Warm {
    /// The warm directory of `identity` under `root`, locked. Refused when
    /// another build holds it.
    pub fn open(root: &Path, identity: &str) -> io::Result<Self> {
        let dir = root.join(identity);
        fs::create_dir_all(&dir)?;
        let lock = fs::File::create(dir.join(LOCK))?;
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).map_err(
            |_| {
                io::Error::other(format!(
                    "another dev build is using {}; dev builds of one identity run one at a time",
                    dir.display()
                ))
            },
        )?;
        Ok(Self { dir, _lock: lock })
    }

    /// Buildroot's output directory in it.
    pub fn out(&self) -> PathBuf {
        self.dir.join("out")
    }

    /// The last build's manifest, when there is one that reads.
    pub fn manifest(&self) -> Option<Manifest> {
        let text = fs::read(self.dir.join(MANIFEST)).ok()?;
        serde_json::from_slice(&text).ok()
    }

    /// Forgets the last build: from now until [`Warm::finish`], the directory
    /// is one a failed build may have left half made.
    pub fn begin(&self) -> io::Result<()> {
        match fs::remove_file(self.dir.join(MANIFEST)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// Empties the output directory.
    pub fn clear(&self) -> io::Result<()> {
        match fs::remove_dir_all(self.out()) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// Records a build that succeeded.
    pub fn finish(&self, manifest: &Manifest) -> io::Result<()> {
        let text = serde_json::to_vec_pretty(manifest).map_err(io::Error::other)?;
        let part = self.dir.join(format!("{MANIFEST}.part"));
        fs::write(&part, text)?;
        fs::rename(part, self.dir.join(MANIFEST))
    }
}

/// The identity's name: a digest of `parts`, each a name and a value.
pub(crate) fn identity(parts: &[(String, String)]) -> String {
    let mut text = String::new();
    for (name, value) in parts {
        text.push_str(&format!("{name}={}\n", value.len()));
        text.push_str(value);
        text.push('\n');
    }
    Digest::of_bytes(text.as_bytes()).hex()
}

/// The fingerprint of a build whose resolved config is `resolved` and whose
/// `make show-info` printed `info`. `places` are the paths to write as names
/// (each input tree's and the source's), with the name each is written as.
pub(crate) fn fingerprint(
    resolved: &str,
    info: &str,
    places: &[(PathBuf, String)],
) -> Result<Manifest, String> {
    let info: serde_json::Value =
        serde_json::from_str(info).map_err(|e| format!("make show-info: {e}"))?;
    let info = info.as_object().ok_or("make show-info printed no object")?;
    // Prefixes of the packages' own lines, longest first.
    let mut owners: Vec<(String, String)> = Vec::new();
    for (name, details) in info {
        if details.get("type").and_then(|t| t.as_str()) == Some("rootfs") {
            continue;
        }
        for prefix in prefixes(name) {
            owners.push((prefix, name.clone()));
        }
    }
    owners.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.cmp(b)));
    let mut packages: BTreeMap<String, Package> = BTreeMap::new();
    for (name, details) in info {
        if details.get("type").and_then(|t| t.as_str()) == Some("rootfs") {
            continue;
        }
        let mut rest = String::new();
        for field in ["version", "downloads", "dependencies", "virtual"] {
            if let Some(value) = details.get(field) {
                rest.push_str(&format!("{field}: {}\n", named(&value.to_string(), places)));
            }
        }
        packages.insert(
            name.clone(),
            Package {
                rest,
                kconfig: String::new(),
            },
        );
    }
    let mut global = Vec::new();
    for line in resolved.lines() {
        let Some(symbol) = symbol(line) else { continue };
        let set = !line.trim_start().starts_with('#');
        let owner = owners
            .iter()
            .find(|(prefix, _)| symbol == prefix || symbol.starts_with(&format!("{prefix}_")))
            .map(|(_, name)| name.clone());
        let written = named(line.trim(), places);
        let named_content = content(line, places);
        match owner {
            Some(name) => {
                let package = packages.entry(name.clone()).or_default();
                let kconfig = KCONFIG
                    .iter()
                    .any(|(owner, options)| *owner == name && options.contains(&symbol));
                let into = if kconfig {
                    &mut package.kconfig
                } else {
                    &mut package.rest
                };
                into.push_str(&written);
                into.push('\n');
                into.push_str(&named_content);
            }
            // A package Buildroot does not build: its lines change nothing
            // built. Enabling one shows as a new package.
            None if is_package_symbol(symbol) => {}
            None if HARMLESS.iter().any(|prefix| symbol.starts_with(prefix)) => {}
            None if !set => global.push(written),
            None => {
                global.push(written);
                if !named_content.is_empty() {
                    global.push(named_content);
                }
            }
        }
    }
    Ok(Manifest { global, packages })
}

/// What to do, from the last build's manifest to this one's.
/// `reverse` gives each package's reverse dependencies in this build.
/// `kconfig_dirclean` removes a Kconfig-only change instead of configuring
/// it again.
pub(crate) fn plan(
    old: Option<&Manifest>,
    new: &Manifest,
    reverse: &BTreeMap<String, Vec<String>>,
    kconfig_dirclean: bool,
) -> Reuse {
    let Some(old) = old else {
        return Reuse::Clean("no earlier dev build of this identity".into());
    };
    if old.global != new.global {
        let gone: Vec<&String> = old
            .global
            .iter()
            .filter(|l| !new.global.contains(l))
            .collect();
        let came: Vec<&String> = new
            .global
            .iter()
            .filter(|l| !old.global.contains(l))
            .collect();
        return Reuse::Clean(format!(
            "a global option changed (was: {}; now: {})",
            short(&gone),
            short(&came)
        ));
    }
    let removed: Vec<&String> = old
        .packages
        .keys()
        .filter(|name| !new.packages.contains_key(*name))
        .collect();
    if !removed.is_empty() {
        return Reuse::Clean(format!("packages removed: {}", short(&removed)));
    }
    let mut dirclean = BTreeSet::new();
    let mut reconfigure = BTreeSet::new();
    for (name, package) in &new.packages {
        match old.packages.get(name) {
            None => {
                dirclean.insert(name.clone());
            }
            Some(before) if before.rest != package.rest => {
                dirclean.insert(name.clone());
            }
            Some(before) if before.kconfig != package.kconfig => {
                if kconfig_dirclean {
                    dirclean.insert(name.clone());
                } else {
                    reconfigure.insert(name.clone());
                }
            }
            Some(_) => {}
        }
    }
    // Everything that depends on a changed package is built again.
    let mut todo: Vec<String> = dirclean.iter().chain(&reconfigure).cloned().collect();
    while let Some(name) = todo.pop() {
        for dependent in reverse.get(&name).into_iter().flatten() {
            if !reconfigure.contains(dependent) && dirclean.insert(dependent.clone()) {
                todo.push(dependent.clone());
            }
        }
    }
    Reuse::Keep {
        dirclean: dirclean.into_iter().collect(),
        reconfigure: reconfigure.into_iter().collect(),
    }
}

/// Each package's reverse dependencies, from `make show-info`.
pub(crate) fn reverse_dependencies(info: &str) -> Result<BTreeMap<String, Vec<String>>, String> {
    let info: serde_json::Value =
        serde_json::from_str(info).map_err(|e| format!("make show-info: {e}"))?;
    let mut reverse = BTreeMap::new();
    for (name, details) in info.as_object().into_iter().flatten() {
        let list = details
            .get("reverse_dependencies")
            .and_then(|r| r.as_array())
            .map(|r| {
                r.iter()
                    .filter_map(|d| d.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        reverse.insert(name.clone(), list);
    }
    Ok(reverse)
}

/// Makes `package`'s config again and keeps what it compiled: its
/// per-package directory is removed, and in its build directory `build_dir`
/// (relative to `out`, as `make show-info` names it) its stamps from the
/// configure on and its `.config`, so Buildroot merges the Kconfig files
/// again and the package's own build rebuilds what changed.
pub(crate) fn reconfigure(out: &Path, package: &str, build_dir: &str) -> io::Result<()> {
    const KEPT: [&str; 5] = [
        ".stamp_downloaded",
        ".stamp_actual_downloaded",
        ".stamp_extracted",
        ".stamp_patched",
        ".stamp_rsynced",
    ];
    match fs::remove_dir_all(out.join("per-package").join(package)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    for file in fs::read_dir(out.join(build_dir))? {
        let file = file?;
        let name = file.file_name().to_string_lossy().into_owned();
        // `.version` is Kbuild's count of links: kept, the next link would be
        // `#2`, where a clean build's is always `#1`.
        if (name.starts_with(".stamp_") && !KEPT.contains(&name.as_str()))
            || name == ".config"
            || name == ".version"
        {
            fs::remove_file(file.path())?;
        }
    }
    Ok(())
}

/// Makes each package install its images again: its `.stamp_images_installed`
/// in each build directory under `out/build` is removed. The images go
/// straight to `out/images`, not to a package's own directory, so once that
/// is emptied, only installing again puts them back.
pub(crate) fn forget_images(out: &Path) -> io::Result<()> {
    let build = out.join("build");
    if !build.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(build)? {
        let entry = entry?;
        // Beside the packages' directories are Buildroot's own files
        // (`build-time.log`, `packages-file-list.txt`).
        if !entry.file_type()?.is_dir() {
            continue;
        }
        match fs::remove_file(entry.path().join(".stamp_images_installed")) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    Ok(())
}

/// A package's build directory, relative to the output directory, from
/// `make show-info`.
pub(crate) fn build_dir(info: &str, package: &str) -> Option<String> {
    let info: serde_json::Value = serde_json::from_str(info).ok()?;
    info.get(package)?
        .get("build_dir")?
        .as_str()
        .map(|dir| dir.trim_end_matches('/').to_string())
}

/// The config symbol of a line, set (`BR2_X=…`) or not (`# BR2_X is not set`).
fn symbol(line: &str) -> Option<&str> {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix("# ") {
        return rest
            .strip_suffix(" is not set")
            .filter(|s| s.starts_with("BR2_"));
    }
    line.split_once('=')
        .map(|(symbol, _)| symbol)
        .filter(|s| s.starts_with("BR2_"))
}

/// Whether a symbol is in a namespace of packages' own options.
fn is_package_symbol(symbol: &str) -> bool {
    symbol.starts_with("BR2_PACKAGE_")
        || symbol.starts_with("BR2_LINUX_KERNEL")
        || (symbol.starts_with("BR2_TARGET_")
            && !symbol.starts_with("BR2_TARGET_GENERIC_")
            && !symbol.starts_with("BR2_TARGET_ROOTFS_")
            && !symbol.starts_with("BR2_TARGET_OPTIMIZATION")
            && !symbol.starts_with("BR2_TARGET_LDFLAGS"))
}

/// The prefixes a package's own options start with.
fn prefixes(name: &str) -> Vec<String> {
    let upper = name.to_uppercase().replace(['-', '.'], "_");
    match name {
        "linux" => vec!["BR2_LINUX_KERNEL".into()],
        _ => vec![
            format!("BR2_PACKAGE_{upper}"),
            format!("BR2_TARGET_{upper}"),
        ],
    }
}

/// `text` with each place written as its name.
fn named(text: &str, places: &[(PathBuf, String)]) -> String {
    let mut text = text.to_string();
    for (path, name) in places {
        let path = path.display().to_string();
        if !path.is_empty() {
            text = text.replace(&path, name);
        }
    }
    text
}

/// The content the value of `line` names: each word that is a path under a
/// place, with the digest of the file or the directory there.
fn content(line: &str, places: &[(PathBuf, String)]) -> String {
    let Some((_, value)) = line.split_once('=') else {
        return String::new();
    };
    let mut named_content = String::new();
    for word in value.trim().trim_matches('"').split_whitespace() {
        let path = Path::new(word);
        if !places.iter().any(|(place, _)| path.starts_with(place)) {
            continue;
        }
        let digest = digest_of(path).unwrap_or_else(|_| "missing".into());
        named_content.push_str(&format!("  {} {digest}\n", named(word, places)));
    }
    named_content
}

/// A digest of a file's bytes, or of a directory's names, modes, links and
/// files, in order.
fn digest_of(path: &Path) -> io::Result<String> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(path)?;
        return Ok(Digest::of_bytes(target.as_os_str().as_encoded_bytes()).hex());
    }
    if meta.is_file() {
        return Ok(Digest::of_bytes(&fs::read(path)?).hex());
    }
    let mut names: Vec<_> = fs::read_dir(path)?
        .map(|entry| entry.map(|e| e.file_name()))
        .collect::<io::Result<_>>()?;
    names.sort();
    let mut text = String::new();
    for name in names {
        use std::os::unix::fs::PermissionsExt;
        let child = path.join(&name);
        let mode = fs::symlink_metadata(&child)?.permissions().mode();
        text.push_str(&format!(
            "{} {mode:o} {}\n",
            name.to_string_lossy(),
            digest_of(&child)?
        ));
    }
    Ok(Digest::of_bytes(text.as_bytes()).hex())
}

/// A few items of a list, for a reason.
fn short<T: AsRef<str>>(items: &[T]) -> String {
    let mut text: Vec<&str> = items.iter().take(4).map(AsRef::as_ref).collect();
    if items.len() > 4 {
        text.push("…");
    }
    if text.is_empty() {
        "nothing".into()
    } else {
        text.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: &str = r#"{
        "busybox": {"type": "target", "name": "busybox", "virtual": false, "version": "1.38.0",
            "downloads": [], "dependencies": ["toolchain"], "reverse_dependencies": []},
        "toolchain": {"type": "target", "name": "toolchain", "virtual": true,
            "dependencies": [], "reverse_dependencies": ["busybox", "zlib"]},
        "zlib": {"type": "target", "name": "zlib", "virtual": false, "version": "1.3",
            "downloads": [], "dependencies": ["toolchain"], "reverse_dependencies": ["util-linux"]},
        "util-linux": {"type": "target", "name": "util-linux", "virtual": false, "version": "2.41",
            "downloads": [], "dependencies": ["toolchain", "zlib"], "reverse_dependencies": []},
        "linux": {"type": "target", "name": "linux", "virtual": false, "version": "7.2.9",
            "downloads": [], "dependencies": ["toolchain"], "reverse_dependencies": []},
        "rootfs-tar": {"type": "rootfs", "name": "tar"}
    }"#;

    fn manifest(resolved: &str, places: &[(PathBuf, String)]) -> Manifest {
        fingerprint(resolved, INFO, places).expect("a fingerprint")
    }

    fn keep(dirclean: &[&str], reconfigure: &[&str]) -> Reuse {
        Reuse::Keep {
            dirclean: dirclean.iter().map(|s| s.to_string()).collect(),
            reconfigure: reconfigure.iter().map(|s| s.to_string()).collect(),
        }
    }

    const BASE: &str = "BR2_aarch64=y\nBR2_PACKAGE_BUSYBOX=y\nBR2_PACKAGE_ZLIB=y\n\
        BR2_PACKAGE_UTIL_LINUX=y\n# BR2_PACKAGE_UTIL_LINUX_MOUNT is not set\n\
        # BR2_PACKAGE_TREE is not set\nBR2_LINUX_KERNEL=y\nBR2_TARGET_ROOTFS_TAR=y\n\
        BR2_TARGET_GENERIC_HOSTNAME=\"one\"\n";

    #[test]
    fn lines_go_to_the_package_they_name_and_the_rest_is_global() {
        let made = manifest(BASE, &[]);
        assert!(
            made.packages["util-linux"]
                .rest
                .contains("UTIL_LINUX_MOUNT")
        );
        assert!(!made.packages["busybox"].rest.contains("UTIL_LINUX"));
        assert!(made.packages["linux"].rest.contains("BR2_LINUX_KERNEL=y"));
        assert_eq!(
            made.global,
            ["BR2_aarch64=y", "BR2_TARGET_GENERIC_HOSTNAME=\"one\""]
        );
    }

    #[test]
    fn the_same_config_reuses_everything() {
        let made = manifest(BASE, &[]);
        let reverse = reverse_dependencies(INFO).expect("reverse");
        assert_eq!(plan(Some(&made), &made, &reverse, false), keep(&[], &[]));
    }

    #[test]
    fn a_package_option_rebuilds_it_and_what_depends_on_it() {
        let reverse = reverse_dependencies(INFO).expect("reverse");
        let old = manifest(BASE, &[]);
        let new = manifest(
            &BASE.replace(
                "BR2_PACKAGE_ZLIB=y",
                "BR2_PACKAGE_ZLIB=y\nBR2_PACKAGE_ZLIB_FAST=y",
            ),
            &[],
        );
        assert_eq!(
            plan(Some(&old), &new, &reverse, false),
            keep(&["util-linux", "zlib"], &[])
        );
    }

    #[test]
    fn a_new_package_is_built_and_nothing_else() {
        let reverse = reverse_dependencies(INFO).expect("reverse");
        let old = manifest(BASE, &[]);
        let mut new = old.clone();
        new.packages.insert("tree".into(), Package::default());
        assert_eq!(
            plan(Some(&old), &new, &reverse, false),
            keep(&["tree"], &[])
        );
    }

    #[test]
    fn a_global_change_a_removed_package_or_no_manifest_builds_clean() {
        let reverse = reverse_dependencies(INFO).expect("reverse");
        let old = manifest(BASE, &[]);
        let new = manifest(&BASE.replace("\"one\"", "\"two\""), &[]);
        assert!(matches!(
            plan(Some(&old), &new, &reverse, false),
            Reuse::Clean(_)
        ));
        let mut fewer = old.clone();
        fewer.packages.remove("busybox");
        assert!(matches!(
            plan(Some(&old), &fewer, &reverse, false),
            Reuse::Clean(_)
        ));
        assert!(matches!(plan(None, &old, &reverse, false), Reuse::Clean(_)));
    }

    #[test]
    fn image_formats_change_nothing_kept() {
        let reverse = reverse_dependencies(INFO).expect("reverse");
        let old = manifest(BASE, &[]);
        let new = manifest(&format!("{BASE}BR2_TARGET_ROOTFS_EXT2=y\n"), &[]);
        assert_eq!(plan(Some(&old), &new, &reverse, false), keep(&[], &[]));
    }

    #[test]
    fn a_kconfig_file_change_configures_the_kernel_again() {
        let scratch = tempfile::tempdir().expect("tempdir");
        let tree = scratch.path().join("board-0123");
        fs::create_dir_all(&tree).expect("tree");
        fs::write(tree.join("trim.config"), "# CONFIG_DUMMY is not set\n").expect("file");
        let places = [(tree.clone(), "@board@".to_string())];
        let resolved = format!(
            "{BASE}BR2_LINUX_KERNEL_CONFIG_FRAGMENT_FILES=\"{}/trim.config\"\n",
            tree.display()
        );
        let reverse = reverse_dependencies(INFO).expect("reverse");
        let old = manifest(&resolved, &places);
        assert!(
            old.packages["linux"]
                .kconfig
                .contains("@board@/trim.config")
        );
        // The same content in another place: nothing changed.
        let moved = scratch.path().join("board-4567");
        fs::create_dir_all(&moved).expect("moved");
        gaia_store::copy_file(tree.join("trim.config"), moved.join("trim.config")).expect("copy");
        let at_moved = manifest(
            &resolved.replace("board-0123", "board-4567"),
            &[(moved.clone(), "@board@".to_string())],
        );
        assert_eq!(plan(Some(&old), &at_moved, &reverse, false), keep(&[], &[]));
        fs::write(tree.join("trim.config"), "CONFIG_DUMMY=y\n").expect("edit");
        let new = manifest(&resolved, &places);
        assert_eq!(
            plan(Some(&old), &new, &reverse, false),
            keep(&[], &["linux"])
        );
        assert_eq!(
            plan(Some(&old), &new, &reverse, true),
            keep(&["linux"], &[])
        );
    }

    #[test]
    fn every_package_installs_its_images_again() {
        let scratch = tempfile::tempdir().expect("tempdir");
        let out = scratch.path();
        for package in ["rpi-firmware-1", "linux-7.2.9", "busybox-1.37"] {
            let dir = out.join("build").join(package);
            fs::create_dir_all(&dir).expect("dir");
            fs::write(dir.join(".stamp_built"), "").expect("stamp");
            if package != "busybox-1.37" {
                fs::write(dir.join(".stamp_images_installed"), "").expect("stamp");
            }
        }
        fs::write(out.join("build/build-time.log"), "").expect("log");
        forget_images(out).expect("forget");
        for package in ["rpi-firmware-1", "linux-7.2.9", "busybox-1.37"] {
            let dir = out.join("build").join(package);
            assert!(!dir.join(".stamp_images_installed").exists(), "{package}");
            assert!(dir.join(".stamp_built").exists(), "{package}");
        }
        forget_images(&out.join("none")).expect("no build directory");
    }

    #[test]
    fn a_reconfigured_package_keeps_its_build_and_loses_its_config() {
        let scratch = tempfile::tempdir().expect("tempdir");
        let out = scratch.path();
        let kernel = out.join("build/linux-7.2.9");
        let headers = out.join("build/linux-headers-7.2.9");
        for dir in [&kernel, &headers, &out.join("per-package/linux/host")] {
            fs::create_dir_all(dir).expect("dir");
        }
        for name in [
            ".stamp_extracted",
            ".stamp_patched",
            ".stamp_configured",
            ".stamp_built",
            ".config",
            ".version",
            "vmlinux.o",
        ] {
            fs::write(kernel.join(name), "").expect("file");
            fs::write(headers.join(name), "").expect("file");
        }
        assert_eq!(
            build_dir(r#"{"linux": {"build_dir": "build/linux-7.2.9/"}}"#, "linux").as_deref(),
            Some("build/linux-7.2.9")
        );
        reconfigure(out, "linux", "build/linux-7.2.9").expect("reconfigure");
        let left = |dir: &Path| {
            let mut names: Vec<String> = fs::read_dir(dir)
                .expect("dir")
                .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };
        assert_eq!(
            left(&kernel),
            [".stamp_extracted", ".stamp_patched", "vmlinux.o"]
        );
        assert_eq!(left(&headers).len(), 7, "another package is not touched");
        assert!(!out.join("per-package/linux").exists());
    }
}
