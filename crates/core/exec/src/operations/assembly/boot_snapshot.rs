//! A copy of each boot tree's boot files, for `image:check`. The firmware
//! rule, the U-Boot files and the hardware list are checked against the
//! trees the board boots from, and a RAM work dir's trees are removed when
//! the assembly ends, so the assembly copies what those checks read (every
//! `*.txt`, `README`, `.dtbo`, `.dtb`, `.conf`, `.scr` and `.itb`, at their
//! paths in the tree) of each boot tree to [`gaia_spec::boot_tree_snapshot_dir`].
//! A boot tree has a top-level `config.txt` (Raspberry Pi), or an
//! `extlinux/extlinux.conf`, a `boot.scr` or an `*.itb` at its root or under
//! `boot/` (U-Boot). The copy also lists every file of the tree in
//! [`BOOT_TREE_FILES_LIST`], so a kernel the U-Boot check looks for is found
//! without copying it.

use super::*;

/// A snapshot tree's list of the original tree's files, one path relative
/// to the tree a line, sorted.
pub(crate) const BOOT_TREE_FILES_LIST: &str = ".gaia-files";

/// Replaces the snapshot with this run's boot trees. The directory is made
/// even when no tree is a boot tree: its presence marks the assembly's
/// outputs complete for reuse.
pub(super) fn write_boot_tree_snapshot(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
) -> Result<(), AssemblyError> {
    let snapshot = gaia_spec::boot_tree_snapshot_dir(spec);
    if snapshot.exists() {
        std_fs::remove_dir_all(&snapshot).map_err(|error| {
            format!(
                "failed to clear the boot tree snapshot '{}': {error}",
                snapshot.display()
            )
        })?;
    }
    std_fs::create_dir_all(&snapshot).map_err(|error| {
        format!(
            "failed to create the boot tree snapshot '{}': {error}",
            snapshot.display()
        )
    })?;
    for (id, tree) in &roots.trees {
        if !is_boot_tree(tree) {
            continue;
        }
        let dest = snapshot.join(id.as_str());
        let mut files = Vec::new();
        copy_boot_files(tree, tree, &dest, &mut files)?;
        files.sort();
        let list = dest.join(BOOT_TREE_FILES_LIST);
        std_fs::create_dir_all(&dest)
            .and_then(|()| std_fs::write(&list, files.join("\n") + "\n"))
            .map_err(|error| format!("failed to write '{}': {error}", list.display()))?;
    }
    Ok(())
}

/// Whether a Raspberry Pi or a U-Boot board boots from `tree`.
fn is_boot_tree(tree: &Path) -> bool {
    if tree.join("config.txt").is_file() {
        return true;
    }
    ["", "boot"].iter().any(|dir| {
        let dir = tree.join(dir);
        dir.join("extlinux/extlinux.conf").is_file()
            || dir.join("boot.scr").is_file()
            || std_fs::read_dir(&dir).is_ok_and(|entries| {
                entries.flatten().any(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .ends_with(".itb")
                })
            })
    })
}

fn copy_boot_files(
    root: &Path,
    dir: &Path,
    dest_root: &Path,
    files: &mut Vec<String>,
) -> Result<(), AssemblyError> {
    let entries = std_fs::read_dir(dir)
        .map_err(|error| format!("failed to read '{}': {error}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if kind.is_dir() {
            copy_boot_files(root, &path, dest_root, files)?;
        } else if kind.is_file() || kind.is_symlink() {
            files.push(relative.to_string_lossy().into_owned());
        }
        if kind.is_file() && is_boot_file(&path) {
            let dest = dest_root.join(relative);
            if let Some(parent) = dest.parent() {
                std_fs::create_dir_all(parent)
                    .map_err(|error| format!("failed to create '{}': {error}", parent.display()))?;
            }
            std_fs::copy(&path, &dest).map_err(|error| {
                format!(
                    "failed to copy '{}' to the boot tree snapshot: {error}",
                    path.display()
                )
            })?;
        }
    }
    Ok(())
}

/// The files the boot checks read: config files, overlay READMEs, overlays,
/// board device trees, U-Boot scripts and FIT images.
fn is_boot_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    name == "readme"
        || [".txt", ".dtbo", ".dtb", ".conf", ".scr", ".itb"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
}
