//! A copy of each boot tree's boot files, for `image:check`. The firmware
//! rule and the hardware list are checked against the trees the board boots
//! from, and a RAM work dir's trees are removed when the assembly ends, so
//! the assembly copies what those checks read (every `*.txt`, `README`,
//! `.dtbo` and `.dtb`, at their paths in the tree) of each tree with a
//! top-level `config.txt` to [`gaia_spec::boot_tree_snapshot_dir`].

use super::*;

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
        if !tree.join("config.txt").is_file() {
            continue;
        }
        copy_boot_files(tree, tree, &snapshot.join(id.as_str()))?;
    }
    Ok(())
}

fn copy_boot_files(root: &Path, dir: &Path, dest_root: &Path) -> Result<(), AssemblyError> {
    let entries = std_fs::read_dir(dir)
        .map_err(|error| format!("failed to read '{}': {error}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            copy_boot_files(root, &path, dest_root)?;
        } else if kind.is_file() && is_boot_file(&path) {
            let relative = path.strip_prefix(root).unwrap_or(&path);
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

/// The files the boot checks read: config files, overlay READMEs, overlays
/// and board device trees.
fn is_boot_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    name == "readme" || name.ends_with(".txt") || name.ends_with(".dtbo") || name.ends_with(".dtb")
}
