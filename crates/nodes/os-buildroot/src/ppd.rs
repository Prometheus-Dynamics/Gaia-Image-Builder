//! Per-package directories (`BR2_PER_PACKAGE_DIRECTORIES`), as the host
//! stage and the OS builds use them.
//!
//! With the option on, Buildroot installs each package into its own
//! `per-package/<package>/host` and `per-package/<package>/target`, and gives
//! a package the trees of its dependencies, so packages can build at once
//! under `make -j`. Without it the top level is `.NOTPARALLEL` and packages
//! build one after another. Gaia turns the option on by default (the
//! `gaia.parallel` fragment in `gaia-graph`).
//!
//! Two things change for Gaia with the option on:
//!
//! - **The merged host directory.** Buildroot assembles `out/host` from the
//!   packages' host trees in `host-finalize`, which every `all` runs before
//!   `world`. A goal build (the host stage's goals are host packages) does
//!   not run it, and `host-finalize` depends on every package of the
//!   configuration, so building it would build the whole configuration. The
//!   host stage therefore merges its own goals' trees here, as Buildroot's
//!   finalize does: [`merge_host`].
//! - **The seed's packages.** A host package the seed holds is installed,
//!   so Buildroot never prepares its per-package directories: it is planted
//!   here, from the seed's `host/`, as [`plant`] does, with an empty
//!   `target/`. Each package's `host/` is then complete, and a build that
//!   depends on it finds it.
//!
//! The merge copies with reflink clones (`cp --reflink=auto`): on a
//! filesystem with reflinks they copy no data, and each copy stays independent
//! of the others. Without reflinks `cp` copies every file.
//!
//! The planted trees are hard links, as Buildroot's own per-package
//! preparation makes them (`rsync --hard-links --link-dest`): a clone of the
//! seed's host for each of its packages is a full copy per package where there
//! are no reflinks (tmpfs), and an LLVM toolchain's host, planted for each of
//! its packages, is past 100 GB. Links share each file, so an in-place edit
//! (stripping, `patchelf`) through one package's tree reaches every other.
//! [`plant`] therefore links them to one clone of the host kept out of
//! Buildroot's reach, `out/gaia-plant/host`, and records the path, mode, size
//! and digest of each file; [`plant_changes`] reads them again when the build is done, and a
//! file written in place shows there whichever tree it was written through.
//! A file a package replaces is a new file in its own tree only, and is not
//! a change. The output directory's own `host/` stays a copy.
//!
//! Buildroot's own `host-finalize` rewrites the paths of the text files it
//! merges: a `<out>/per-package/<package>/` prefix becomes `<out>/`
//! (`ppd-fixup-paths` in `package/pkg-utils.mk`). [`fix_paths`] does the same
//! to the merged host directory, so the seed holds no per-package path in a
//! text file.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;

use crate::host::{files, write_keeping_mode};

/// The line of a resolved `.config` that turns per-package directories on.
const ON: &str = "BR2_PER_PACKAGE_DIRECTORIES=y";
/// The seed's list of the packages it holds, one name per line: the goals
/// the host stage built.
pub(crate) const PACKAGES_FILE: &str = "packages";

/// Whether the resolved `.config` (Buildroot's own text) has per-package
/// directories on.
pub(crate) fn enabled(resolved: &str) -> bool {
    resolved.lines().any(|line| line.trim() == ON)
}

/// Copies every tree of `packages` under `out/per-package/` that has a
/// `host/` into `out/host`, in name order: a file two trees hold is the
/// later one's. Missing trees are skipped (a package that installed nothing
/// has none). Then the paths in the merged text files are fixed: they name
/// `seen`, the path the build saw `out` at. Creates `out/host` when there is
/// no tree at all.
pub(crate) fn merge_host(out: &Path, seen: &Path, packages: &[&str]) -> io::Result<()> {
    let host = out.join("host");
    fs::create_dir_all(&host)?;
    let mut names: Vec<&str> = packages.to_vec();
    names.sort_unstable();
    names.dedup();
    for name in names {
        let tree = out.join("per-package").join(name).join("host");
        if tree.is_dir() {
            copy_into(&tree, &host)?;
        }
    }
    fix_text_files(&host, seen)
}

/// Where [`plant`] keeps its reference links and their manifest, in the
/// output directory.
pub(crate) const PLANT_DIR: &str = "gaia-plant";
const PLANT_MANIFEST: &str = "manifest.json";

/// One planted file as it was planted: mode, size and digest.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Planted {
    mode: u32,
    size: u64,
    digest: String,
}

/// Plants per-package directories for each of `packages` from `host`, the
/// seed's (or the merged) host directory: `host` is cloned once to
/// `out/gaia-plant/host`, with a manifest of its files, and each
/// `per-package/<package>/host` is hard links to that clone, with an empty
/// `target/`. `out` is the output directory. `host` stays independent of
/// them all (see the module's docs); [`plant_changes`] reads them again. Returns the number
/// of regular files the manifest names.
pub(crate) fn plant(host: &Path, out: &Path, packages: &[String]) -> io::Result<usize> {
    if packages.is_empty() {
        return Ok(0);
    }
    let reference = out.join(PLANT_DIR);
    fs::create_dir_all(&reference)?;
    // One clone of `host`, which stays the output directory's own: the
    // packages' trees are links to the clone.
    let linked = reference.join("host");
    run_cp(&["-a", "--reflink=auto"], host, &linked)?;
    let manifest = manifest_of(&linked)?;
    let text = serde_json::to_vec(&manifest).map_err(io::Error::other)?;
    fs::write(reference.join(PLANT_MANIFEST), text)?;
    for name in packages {
        let dir = out.join("per-package").join(name);
        fs::create_dir_all(dir.join("target"))?;
        link_tree(&linked, &dir.join("host"))?;
    }
    Ok(manifest.len())
}

/// The planted files that were changed in place since [`plant`], each
/// named with what changed. Empty when nothing was planted in `out`.
pub(crate) fn plant_changes(out: &Path) -> io::Result<Vec<String>> {
    let reference = out.join(PLANT_DIR);
    let text = match fs::read(reference.join(PLANT_MANIFEST)) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let then: BTreeMap<String, Planted> =
        serde_json::from_slice(&text).map_err(io::Error::other)?;
    let now = manifest_of(&reference.join("host"))?;
    let mut changed = Vec::new();
    for (path, was) in &then {
        match now.get(path) {
            None => changed.push(format!("{path}: gone")),
            Some(is) if is.mode != was.mode => {
                changed.push(format!("{path}: mode {:o} -> {:o}", was.mode, is.mode));
            }
            Some(is) if is.size != was.size || is.digest != was.digest => {
                changed.push(format!(
                    "{path}: bytes changed ({} -> {} bytes)",
                    was.size, is.size
                ));
            }
            Some(_) => {}
        }
    }
    Ok(changed)
}

/// The regular files under `root`, each with its mode, size and digest.
/// Hashed on as many threads as the machine allows.
fn manifest_of(root: &Path) -> io::Result<BTreeMap<String, Planted>> {
    let mut regular = Vec::new();
    for relative in files(root)? {
        let meta = fs::symlink_metadata(root.join(&relative))?;
        if meta.file_type().is_file() {
            regular.push(relative);
        }
    }
    let threads = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(regular.len().max(1));
    let chunk = regular.len().div_ceil(threads).max(1);
    let parts = std::thread::scope(|scope| {
        let running: Vec<_> = regular
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || -> io::Result<Vec<(String, Planted)>> {
                    let mut found = Vec::with_capacity(part.len());
                    for relative in part {
                        let path = root.join(relative);
                        let meta = fs::symlink_metadata(&path)?;
                        let digest = gaia_store::file_digest(&path)
                            .map_err(io::Error::other)?
                            .hex();
                        found.push((
                            relative.display().to_string(),
                            Planted {
                                mode: meta.mode() & 0o7777,
                                size: meta.len(),
                                digest,
                            },
                        ));
                    }
                    Ok(found)
                })
            })
            .collect();
        running
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| Err(io::Error::other("a hashing thread panicked")))
            })
            .collect::<Vec<_>>()
    });
    let mut manifest = BTreeMap::new();
    for part in parts {
        manifest.extend(part?);
    }
    Ok(manifest)
}

/// The bytes a build's work directory holds once `seed` is planted, before
/// anything builds: the seed's `host/` once in the output directory's own
/// `host/`, and once more for the clone the seed's packages link to (see
/// [`plant`]); the links take no more. A copy takes every byte where there
/// are no reflinks, as in RAM, so this is the size with none. Counted as if
/// per-package directories are on, as Gaia's default is: the plant is known
/// before the build's config is resolved. A seed with no host plants
/// nothing, and one with no packages only its `host/`.
pub(crate) fn plant_bytes(seed: &Path) -> io::Result<u64> {
    let host = seed.join("host");
    if !host.is_dir() {
        return Ok(0);
    }
    let packages = match read_packages(seed) {
        Ok(packages) => packages.len() as u64,
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    let mut bytes = 0_u64;
    for relative in files(&host)? {
        bytes = bytes.saturating_add(fs::symlink_metadata(host.join(relative))?.len());
    }
    Ok(bytes.saturating_mul(if packages == 0 { 1 } else { 2 }))
}

/// The packages the seed holds, from its `packages` file.
pub(crate) fn read_packages(seed: &Path) -> io::Result<Vec<String>> {
    Ok(fs::read_to_string(seed.join(PACKAGES_FILE))?
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// `text` with each `<out>/per-package/<package>/` replaced by `<out>/`, as
/// Buildroot's `ppd-fixup-paths` does (`s:PER_PACKAGE_DIR/[^/]\+/:BASE/:g`,
/// per line). A prefix with no package name after it is left as it is.
pub(crate) fn fix_paths(text: &str, out: &str) -> String {
    let prefix = format!("{out}/per-package/");
    let mut fixed = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(&prefix) {
        fixed.push_str(&rest[..at]);
        let after = &rest[at + prefix.len()..];
        match after.find('/') {
            Some(end) if end > 0 && !after[..end].contains('\n') => {
                fixed.push_str(out);
                fixed.push('/');
                rest = &after[end + 1..];
            }
            _ => {
                fixed.push_str(&prefix);
                rest = after;
            }
        }
    }
    fixed.push_str(rest);
    fixed
}

/// Fixes the paths in each regular text file under `host`, keeping its mode.
/// Binaries and symlinks are left as Buildroot leaves them.
fn fix_text_files(host: &Path, out: &Path) -> io::Result<()> {
    let out = out.display().to_string();
    let prefix = format!("{out}/per-package/");
    for relative in files(host)? {
        let path = host.join(&relative);
        let meta = fs::symlink_metadata(&path)?;
        if !meta.file_type().is_file() {
            continue;
        }
        let bytes = fs::read(&path)?;
        if bytes.contains(&0) {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        if text.contains(&prefix) {
            write_keeping_mode(&path, &meta, fix_paths(&text, &out).as_bytes())?;
        }
    }
    Ok(())
}

/// Copies the contents of `from` into `to` (which exists), replacing each
/// file there, as clones.
fn copy_into(from: &Path, to: &Path) -> io::Result<()> {
    let from = from.join(".");
    let to = to.join(".");
    run_cp(
        &["-a", "--reflink=auto", "--remove-destination"],
        &from,
        &to,
    )
}

/// Hard links of the tree `from` at `to`, which must not exist: new
/// directories, every file and symlink a link to `from`'s.
fn link_tree(from: &Path, to: &Path) -> io::Result<()> {
    run_cp(&["-a", "--link"], from, to)
}

fn run_cp(options: &[&str], from: &Path, to: &Path) -> io::Result<()> {
    let status = Command::new("cp")
        .args(options)
        .arg(from)
        .arg(to)
        .env("PATH", crate::PATH)
        .env("LC_ALL", "C")
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "cp {} {}: {status}",
            from.display(),
            to.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    /// A fake output directory with the per-package trees a make would have
    /// left. Each tree is a `host/` with its files.
    fn out_with(trees: &[(&str, &[(&str, &str)])]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (name, files) in trees {
            for (relative, text) in files.iter() {
                let path = dir
                    .path()
                    .join("per-package")
                    .join(name)
                    .join("host")
                    .join(relative);
                fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
                fs::write(&path, text).expect("file");
            }
        }
        dir
    }

    #[test]
    fn the_option_is_read_from_the_resolved_config() {
        assert!(enabled("BR2_x86_64=y\nBR2_PER_PACKAGE_DIRECTORIES=y\n"));
        assert!(enabled("  BR2_PER_PACKAGE_DIRECTORIES=y  \n"));
        assert!(!enabled("# BR2_PER_PACKAGE_DIRECTORIES is not set\n"));
        assert!(!enabled("BR2_PER_PACKAGE_DIRECTORIES=yes\n"));
        assert!(!enabled(""));
    }

    /// The text of a per-package path becomes the output directory's, and
    /// nothing else changes: a name with no following slash, an empty name
    /// and a path of another directory are left alone.
    #[test]
    fn a_per_package_prefix_is_fixed_to_the_output_directory() {
        let out = "/o";
        assert_eq!(
            fix_paths("prefix=/o/per-package/host-pkgconf/host/usr\n", out),
            "prefix=/o/host/usr\n"
        );
        assert_eq!(
            fix_paths("a=/o/per-package/x/host/a /o/per-package/y/target/b", out),
            "a=/o/host/a /o/target/b"
        );
        assert_eq!(
            fix_paths("/o/per-package//host", out),
            "/o/per-package//host"
        );
        assert_eq!(fix_paths("/o/per-package/host", out), "/o/per-package/host");
        assert_eq!(
            fix_paths("/other/per-package/x/host", out),
            "/other/per-package/x/host"
        );
    }

    /// Each package's host tree is merged into `out/host`, the later name
    /// winning a shared file, with the paths fixed and the modes kept.
    #[test]
    fn merged_host_has_every_tree_and_the_later_package_wins_a_shared_file() {
        let dir = out_with(&[
            ("host-a", &[("bin/a", "#!/x\n"), ("share.txt", "a")]),
            ("host-b", &[("share.txt", "b"), ("lib/b.so", "\u{7f}ELF")]),
        ]);
        let out = dir.path();
        let script = out.join("per-package/host-a/host/bin/a");
        fs::write(
            &script,
            format!("#!{}/per-package/host-a/host/bin/sh\n", out.display()),
        )
        .expect("script");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("mode");
        merge_host(out, out, &["host-b", "host-a", "host-missing"]).expect("merge");
        let host = out.join("host");
        assert_eq!(
            fs::read_to_string(host.join("share.txt")).expect("share"),
            "b"
        );
        assert!(host.join("lib/b.so").is_file());
        let text = fs::read_to_string(host.join("bin/a")).expect("bin");
        assert_eq!(text, format!("#!{}/host/bin/sh\n", out.display()));
        let mode = fs::metadata(host.join("bin/a"))
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    /// With no tree for any package, the merged host is an empty directory.
    #[test]
    fn a_merge_with_no_tree_gives_an_empty_host() {
        let dir = tempfile::tempdir().expect("tempdir");
        merge_host(dir.path(), dir.path(), &["host-a"]).expect("merge");
        assert!(dir.path().join("host").is_dir());
        assert!(files(&dir.path().join("host")).expect("list").is_empty());
    }

    /// Planting gives each seeded package a complete host tree and an empty
    /// target tree, and the seed's stamps are left as they are.
    #[test]
    fn a_planted_package_has_the_seed_host_and_an_empty_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path();
        let host = out.join("host");
        fs::create_dir_all(host.join("bin")).expect("host");
        fs::write(host.join("bin/tool"), "tool").expect("tool");
        let packages = vec!["host-a".to_string(), "host-b".to_string()];
        plant(&host, out, &packages).expect("plant");
        for name in ["host-a", "host-b"] {
            let base = out.join("per-package").join(name);
            assert_eq!(
                fs::read_to_string(base.join("host/bin/tool")).expect("tool"),
                "tool"
            );
            assert!(base.join("target").is_dir());
            assert!(files(&base.join("target")).expect("list").is_empty());
        }
        // The packages' trees are one set of files, not a copy each: the
        // space a plant takes does not grow with the number of packages.
        let inode = |name: &str| {
            fs::metadata(out.join("per-package").join(name).join("host/bin/tool"))
                .expect("tool")
                .ino()
        };
        assert_eq!(inode("host-a"), inode("host-b"));
        // The output directory's own host is not one of them: an edit
        // through a package's tree does not reach it.
        fs::write(out.join("per-package/host-a/host/bin/tool"), "edited").expect("edit");
        assert_eq!(
            fs::read_to_string(host.join("bin/tool")).expect("tool"),
            "tool"
        );
    }

    /// A planted output directory: `host/bin/tool` and `host/lib/data`,
    /// planted for `host-a` and `host-b`.
    fn planted() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let host = dir.path().join("host");
        fs::create_dir_all(host.join("bin")).expect("bin");
        fs::create_dir_all(host.join("lib")).expect("lib");
        fs::write(host.join("bin/tool"), "tool").expect("tool");
        fs::set_permissions(host.join("bin/tool"), fs::Permissions::from_mode(0o755))
            .expect("mode");
        fs::write(host.join("lib/data"), "data").expect("data");
        std::os::unix::fs::symlink("tool", host.join("bin/alias")).expect("link");
        let packages = vec!["host-a".to_string(), "host-b".to_string()];
        assert_eq!(plant(&host, dir.path(), &packages).expect("plant"), 2);
        dir
    }

    #[test]
    fn an_untouched_plant_has_no_changes() {
        let dir = planted();
        assert!(plant_changes(dir.path()).expect("check").is_empty());
    }

    #[test]
    fn a_write_in_place_through_one_package_is_named() {
        let dir = planted();
        // As `patchelf` or `strip` would: the same file, opened and written.
        let mut file = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(dir.path().join("per-package/host-b/host/bin/tool"))
            .expect("open");
        std::io::Write::write_all(&mut file, b"stripped").expect("write");
        drop(file);
        assert_eq!(
            plant_changes(dir.path()).expect("check"),
            ["bin/tool: bytes changed (4 -> 8 bytes)"]
        );
    }

    #[test]
    fn a_mode_changed_through_one_package_is_named() {
        let dir = planted();
        fs::set_permissions(
            dir.path().join("per-package/host-a/host/lib/data"),
            fs::Permissions::from_mode(0o600),
        )
        .expect("mode");
        assert_eq!(
            plant_changes(dir.path()).expect("check"),
            ["lib/data: mode 644 -> 600"]
        );
    }

    #[test]
    fn a_file_a_package_replaces_is_its_own_and_no_change() {
        let dir = planted();
        let tool = dir.path().join("per-package/host-a/host/bin/tool");
        fs::remove_file(&tool).expect("remove");
        fs::write(&tool, "rebuilt").expect("write");
        assert!(plant_changes(dir.path()).expect("check").is_empty());
        assert_eq!(
            fs::read_to_string(dir.path().join("per-package/host-b/host/bin/tool")).expect("tool"),
            "tool"
        );
    }

    #[test]
    fn a_plant_counts_the_seed_host_twice_whatever_the_packages() {
        let dir = tempfile::tempdir().expect("tempdir");
        let seed = dir.path();
        assert_eq!(plant_bytes(seed).expect("no host"), 0);
        fs::create_dir_all(seed.join("host/bin")).expect("host");
        fs::write(seed.join("host/bin/tool"), vec![0_u8; 1000]).expect("tool");
        fs::write(seed.join("host/bin/lib"), vec![0_u8; 24]).expect("lib");
        assert_eq!(plant_bytes(seed).expect("no packages file"), 1024);
        fs::write(seed.join(PACKAGES_FILE), "host-a\nhost-b\nhost-c\n").expect("packages");
        // The host, and the one clone the packages link to.
        assert_eq!(plant_bytes(seed).expect("three packages"), 2 * 1024);
        fs::write(seed.join(PACKAGES_FILE), "host-a\n".repeat(50)).expect("packages");
        assert_eq!(plant_bytes(seed).expect("fifty packages"), 2 * 1024);
    }

    #[test]
    fn a_directory_with_nothing_planted_is_not_checked() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(plant_changes(dir.path()).expect("check").is_empty());
        let host = dir.path().join("host");
        fs::create_dir_all(&host).expect("host");
        assert_eq!(plant(&host, dir.path(), &[]).expect("plant"), 0);
        assert!(!dir.path().join(PLANT_DIR).exists());
    }

    #[test]
    fn the_packages_file_is_read_as_one_name_per_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join(PACKAGES_FILE), "host-a\nhost-b\n\n").expect("file");
        assert_eq!(
            read_packages(dir.path()).expect("read"),
            ["host-a", "host-b"]
        );
    }
}
