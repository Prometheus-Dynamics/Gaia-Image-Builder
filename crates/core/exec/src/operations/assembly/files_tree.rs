//! A `src_dir` file entry: the whole tree under a directory copied into a
//! tree under `dest`. Entries are walked in sorted order, a directory before
//! its contents; symbolic links are kept as links and never followed; a
//! regular file keeps its permission bits unless `mode` is set. Each copied
//! regular file gets the same strip and runtime-library check as a `src`
//! copy.

use super::steps::{PendingLibCheck, Record, StepRun};
use super::steps_jobs::JobCtx;
use super::*;

/// One entry under a `src_dir`, by its path relative to it.
enum TreeEntry {
    Dir { rel: PathBuf },
    Link { rel: PathBuf, target: String },
    File { rel: PathBuf, src: PathBuf },
}

/// What copying a `src_dir` made.
#[derive(Default)]
struct TreeCopy {
    dirs: usize,
    links: usize,
    /// The copied regular files, under the destination directory.
    files: Vec<PathBuf>,
}

/// The work of a `src_dir` file entry. `file` is the entry of `entry_index`,
/// which the caller checked names a `src_dir`.
pub(super) fn job_file_tree<'a>(
    ctx: &JobCtx<'a>,
    entry_index: usize,
    file: &'a gaia_spec::AssemblyFileSpec,
) -> Result<Record<'a>, AssemblyError> {
    let Some(src_dir) = &file.src_dir else {
        return Err(format!("assembly file entry {entry_index} sets no src_dir").into());
    };
    let tree_path = ctx.roots.tree_path(&file.tree)?;
    let root = ctx.roots.resolve_path(ctx.spec, src_dir)?;
    let dest_dir = assembly_tree_dest(tree_path, &file.dest)?;
    let entries = match std_fs::metadata(&root) {
        Err(_) => Vec::new(),
        Ok(metadata) if !metadata.is_dir() => {
            return Err(format!("assembly src_dir '{}' is not a directory", root.display()).into());
        }
        Ok(_) => list_tree_entries(&root, &file.exclude)?,
    };
    if entries.is_empty() {
        if !file.optional {
            return Err(format!(
                "assembly src_dir '{}' is missing or has no entries to copy",
                root.display()
            )
            .into());
        }
        return Ok(Box::new(move |run: &mut StepRun<'a>| {
            run.skipped_count += 1;
            run.state
                .insert(format!("file.skipped.{entry_index}.empty"), "true");
            run.state
                .insert(format!("file.skipped.{entry_index}.tree"), &file.tree);
        }));
    }
    let mut copy = copy_tree(&root, &dest_dir, &entries, file)?;
    let strip = if file.strip && !copy.files.is_empty() {
        let mut stripper = Stripper::new(
            ctx.spec,
            ctx.roots,
            ctx.assembly.strip_tool.as_ref(),
            ctx.cancel.clone(),
        );
        Some(stripper.strip_paths(&copy.files)?)
    } else {
        None
    };
    // A tree a package with device checks is made from relies on the
    // device's own libraries; the package step checks them instead.
    let device_checked = ctx
        .assembly
        .packages
        .iter()
        .any(|package| package.tree == file.tree && package.device.is_some());
    let pending: Vec<PendingLibCheck> = if file.allow_missing_libs || device_checked {
        Vec::new()
    } else {
        copy.files
            .iter()
            .filter(|dest| gaia_plan::is_dynamic_elf(dest))
            .map(|dest| PendingLibCheck {
                tree: file.tree.as_str().to_string(),
                tree_path: tree_path.to_path_buf(),
                file: dest.clone(),
            })
            .collect()
    };
    let mut bytes = 0u64;
    for dest in &copy.files {
        bytes += file_len(dest)?;
    }
    let files = copy.files.len();
    copy.files.clear();
    let (dirs, links) = (copy.dirs, copy.links);
    let root_display = root.display().to_string();
    let dest_display = dest_dir.display().to_string();
    let mode = file.mode.clone();
    Ok(Box::new(move |run: &mut StepRun<'a>| {
        if let Some(strip) = strip {
            run.strip.add(strip);
        }
        run.pending_lib_checks.extend(pending);
        run.staged_count += files;
        run.state
            .insert(format!("file.tree.{entry_index}.src_dir"), root_display);
        run.state
            .insert(format!("file.tree.{entry_index}.dest"), dest_display);
        run.state
            .insert(format!("file.tree.{entry_index}.files"), files);
        run.state
            .insert(format!("file.tree.{entry_index}.dirs"), dirs);
        run.state
            .insert(format!("file.tree.{entry_index}.links"), links);
        run.state
            .insert(format!("file.tree.{entry_index}.bytes"), bytes);
        if let Some(mode) = &mode {
            run.state
                .insert(format!("file.tree.{entry_index}.mode"), mode);
        }
    }))
}

/// Every entry under `root` that `exclude` does not drop, in sorted order,
/// a directory before its contents. An excluded entry is left out by its
/// file name at any depth; an excluded directory takes its contents with it.
fn list_tree_entries(root: &Path, exclude: &[String]) -> Result<Vec<TreeEntry>, String> {
    let mut entries = Vec::new();
    walk_tree(root, Path::new(""), exclude, &mut entries)?;
    Ok(entries)
}

fn walk_tree(
    root: &Path,
    rel_dir: &Path,
    exclude: &[String],
    entries: &mut Vec<TreeEntry>,
) -> Result<(), String> {
    let dir = root.join(rel_dir);
    let read = std_fs::read_dir(&dir)
        .map_err(|error| format!("failed to read '{}': {error}", dir.display()))?;
    let mut children = Vec::new();
    for entry in read {
        children.push(
            entry
                .map_err(|error| format!("failed to read '{}': {error}", dir.display()))?
                .path(),
        );
    }
    children.sort();
    for child in children {
        let name = child
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("'{}' has no UTF-8 file name", child.display()))?
            .to_string();
        if exclude
            .iter()
            .any(|pattern| gaia_spec::wildcard_match(pattern, &name))
        {
            continue;
        }
        let rel = rel_dir.join(&name);
        let metadata = std_fs::symlink_metadata(&child)
            .map_err(|error| format!("failed to read '{}': {error}", child.display()))?;
        let file_type = metadata.file_type();
        if file_type.is_dir() {
            entries.push(TreeEntry::Dir { rel: rel.clone() });
            walk_tree(root, &rel, exclude, entries)?;
        } else if file_type.is_symlink() {
            let target = std_fs::read_link(&child)
                .map_err(|error| format!("failed to read '{}': {error}", child.display()))?;
            let target = target
                .to_str()
                .ok_or_else(|| format!("symlink '{}' target is not UTF-8", child.display()))?
                .to_string();
            entries.push(TreeEntry::Link { rel, target });
        } else if file_type.is_file() {
            entries.push(TreeEntry::File { rel, src: child });
        } else {
            return Err(format!(
                "'{}' under src_dir is neither a file, a directory nor a symbolic link",
                child.display()
            ));
        }
    }
    Ok(())
}

/// Copies the entries under `dest_dir`, which is merged into when it exists.
fn copy_tree(
    root: &Path,
    dest_dir: &Path,
    entries: &[TreeEntry],
    file: &gaia_spec::AssemblyFileSpec,
) -> Result<TreeCopy, String> {
    let mode = file.parsed_mode().map_err(|error| error.to_string())?;
    std_fs::create_dir_all(dest_dir).map_err(|error| {
        format!(
            "failed to create assembly src_dir destination '{}': {error}",
            dest_dir.display()
        )
    })?;
    let mut copy = TreeCopy::default();
    for entry in entries {
        match entry {
            TreeEntry::Dir { rel } => {
                let dest = dest_dir.join(rel);
                clear_for_copy(&dest, true)?;
                std_fs::create_dir_all(&dest).map_err(|error| {
                    format!(
                        "failed to create assembly directory '{}': {error}",
                        dest.display()
                    )
                })?;
                copy.dirs += 1;
            }
            TreeEntry::Link { rel, target } => {
                let dest = dest_dir.join(rel);
                clear_for_copy(&dest, false)?;
                create_parent(&dest)?;
                create_link(target, &dest)?;
                copy.links += 1;
            }
            TreeEntry::File { rel, src } => {
                let dest = dest_dir.join(rel);
                clear_for_copy(&dest, false)?;
                create_parent(&dest)?;
                std_fs::copy(src, &dest).map_err(|error| {
                    format!(
                        "failed to copy assembly file '{}' from src_dir '{}' to '{}': {error}",
                        src.display(),
                        root.display(),
                        dest.display()
                    )
                })?;
                apply_mode(&dest, mode)?;
                copy.files.push(dest);
            }
        }
    }
    Ok(copy)
}

/// Makes `dest` free for a copy: an existing file or link is removed, and an
/// existing directory is kept for a directory copy but refused for a file.
fn clear_for_copy(dest: &Path, want_dir: bool) -> Result<(), String> {
    let Ok(metadata) = std_fs::symlink_metadata(dest) else {
        return Ok(());
    };
    if metadata.is_dir() {
        if want_dir {
            return Ok(());
        }
        return Err(format!(
            "assembly src_dir copy would write '{}', which is a directory",
            dest.display()
        ));
    }
    std_fs::remove_file(dest).map_err(|error| {
        format!(
            "failed to replace existing assembly entry '{}': {error}",
            dest.display()
        )
    })
}

fn create_parent(dest: &Path) -> Result<(), String> {
    match dest.parent() {
        Some(parent) => std_fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create assembly destination dir '{}': {error}",
                parent.display()
            )
        }),
        None => Ok(()),
    }
}

#[cfg(unix)]
fn create_link(target: &str, dest: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(target, dest).map_err(|error| {
        format!(
            "failed to create assembly symlink '{}' -> '{}': {error}",
            dest.display(),
            target
        )
    })
}

#[cfg(not(unix))]
fn create_link(target: &str, dest: &Path) -> Result<(), String> {
    Err(format!(
        "src_dir symlink '{}' -> '{}' needs a unix host",
        dest.display(),
        target
    ))
}
