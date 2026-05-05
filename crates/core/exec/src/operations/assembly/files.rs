use super::*;
use std::io::Read;

pub(super) fn assembly_file_sources(
    spec: &ResolvedBuildSpec,
    roots: &AssemblyRoots,
    file: &gaia_spec::AssemblyFileSpec,
) -> Result<Vec<PathBuf>, String> {
    if let Some(src) = &file.src {
        return Ok(vec![roots.resolve_path(spec, src)?]);
    }
    let Some(src_glob) = &file.src_glob else {
        return Ok(Vec::new());
    };
    gaia_spec::expand_simple_glob(spec, roots, src_glob)
}

pub(super) fn assembly_file_dest(
    tree_path: &Path,
    source: &Path,
    dest: &str,
) -> Result<PathBuf, String> {
    let source_name = source.file_name().ok_or_else(|| {
        format!(
            "assembly source '{}' has no file name for destination '{}'",
            source.display(),
            dest
        )
    })?;
    let dest_path = if dest == "." || dest.ends_with('/') {
        tree_path.join(dest).join(source_name)
    } else if dest.contains('*') {
        return Err(format!("assembly destination '{dest}' cannot contain '*'"));
    } else {
        tree_path.join(dest)
    };
    let normalized_tree = normalize_path_lossy(tree_path);
    let normalized_dest = normalize_path_lossy(&dest_path);
    if !normalized_dest.starts_with(&normalized_tree) {
        return Err(format!(
            "assembly destination '{}' escapes tree '{}'",
            normalized_dest.display(),
            normalized_tree.display()
        ));
    }
    Ok(normalized_dest)
}

pub(super) fn assembly_tree_dest(tree_path: &Path, dest: &str) -> Result<PathBuf, String> {
    if dest.trim().is_empty() {
        return Err("assembly destination path cannot be empty".into());
    }
    if dest.contains('*') {
        return Err(format!("assembly destination '{dest}' cannot contain '*'"));
    }
    let dest_path = tree_path.join(dest);
    let normalized_tree = normalize_path_lossy(tree_path);
    let normalized_dest = normalize_path_lossy(&dest_path);
    if !normalized_dest.starts_with(&normalized_tree) {
        return Err(format!(
            "assembly destination '{}' escapes tree '{}'",
            normalized_dest.display(),
            normalized_tree.display()
        ));
    }
    Ok(normalized_dest)
}

pub(super) fn create_assembly_dir(
    tree_path: &Path,
    dir: &gaia_spec::AssemblyDirSpec,
) -> Result<PathBuf, String> {
    let dest = assembly_tree_dest(tree_path, &dir.path)?;
    std_fs::create_dir_all(&dest).map_err(|error| {
        format!(
            "failed to create assembly dir '{}' in tree '{}': {error}",
            dest.display(),
            tree_path.display()
        )
    })?;
    apply_mode(&dest, dir.parsed_mode().map_err(|error| error.to_string())?)?;
    Ok(dest)
}

pub(super) fn create_assembly_symlink(
    tree_path: &Path,
    symlink: &gaia_spec::AssemblySymlinkSpec,
) -> Result<PathBuf, String> {
    if symlink.target.trim().is_empty() {
        return Err(format!(
            "assembly symlink '{}' target cannot be empty",
            symlink.path
        ));
    }
    let dest = assembly_tree_dest(tree_path, &symlink.path)?;
    if let Some(parent) = dest.parent() {
        std_fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create assembly symlink parent '{}': {error}",
                parent.display()
            )
        })?;
    }
    if dest.exists() || dest.symlink_metadata().is_ok() {
        let metadata = std_fs::symlink_metadata(&dest).map_err(|error| {
            format!(
                "failed to inspect existing assembly symlink destination '{}': {error}",
                dest.display()
            )
        })?;
        if metadata.file_type().is_dir() {
            std_fs::remove_dir_all(&dest).map_err(|error| {
                format!(
                    "failed to replace existing assembly directory '{}': {error}",
                    dest.display()
                )
            })?;
        } else {
            std_fs::remove_file(&dest).map_err(|error| {
                format!(
                    "failed to replace existing assembly symlink destination '{}': {error}",
                    dest.display()
                )
            })?;
        }
    }
    create_symlink(&symlink.target, &dest)?;
    Ok(dest)
}

#[cfg(unix)]
fn create_symlink(target: &str, dest: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(target, dest).map_err(|error| {
        format!(
            "failed to create assembly symlink '{}' -> '{}': {error}",
            dest.display(),
            target
        )
    })
}

#[cfg(not(unix))]
fn create_symlink(target: &str, dest: &Path) -> Result<(), String> {
    std_fs::write(dest, target).map_err(|error| {
        format!(
            "failed to materialize assembly symlink placeholder '{}' -> '{}': {error}",
            dest.display(),
            target
        )
    })
}

pub(super) fn copy_assembly_file(
    source: &Path,
    dest: &Path,
    file: &gaia_spec::AssemblyFileSpec,
) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std_fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create assembly destination dir '{}': {error}",
                parent.display()
            )
        })?;
    }
    let metadata = std_fs::symlink_metadata(source).map_err(|error| {
        format!(
            "failed to read assembly source metadata '{}': {error}",
            source.display()
        )
    })?;
    if metadata.file_type().is_symlink() && file.preserve_symlink {
        copy_symlink(source, dest)?;
    } else if metadata.is_file() || metadata.file_type().is_symlink() {
        std_fs::copy(source, dest).map_err(|error| {
            format!(
                "failed to copy assembly file '{}' to '{}': {error}",
                source.display(),
                dest.display()
            )
        })?;
    } else {
        return Err(format!(
            "assembly source '{}' is not a file",
            source.display()
        ));
    }
    apply_mode(dest, file.parsed_mode().map_err(|error| error.to_string())?)?;
    Ok(())
}

#[cfg(unix)]
fn copy_symlink(source: &Path, dest: &Path) -> Result<(), String> {
    let target = std_fs::read_link(source).map_err(|error| {
        format!(
            "failed to read assembly symlink '{}': {error}",
            source.display()
        )
    })?;
    if dest.exists() {
        std_fs::remove_file(dest).map_err(|error| {
            format!(
                "failed to replace assembly symlink '{}': {error}",
                dest.display()
            )
        })?;
    }
    std::os::unix::fs::symlink(&target, dest).map_err(|error| {
        format!(
            "failed to create assembly symlink '{}' -> '{}': {error}",
            dest.display(),
            target.display()
        )
    })
}

#[cfg(not(unix))]
fn copy_symlink(source: &Path, dest: &Path) -> Result<(), String> {
    std_fs::copy(source, dest).map(|_| ()).map_err(|error| {
        format!(
            "failed to copy assembly symlink target '{}' to '{}': {error}",
            source.display(),
            dest.display()
        )
    })
}

#[cfg(unix)]
pub(super) fn apply_mode(dest: &Path, mode: Option<gaia_spec::FileMode>) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let Some(mode) = mode else {
        return Ok(());
    };
    let mut permissions = std_fs::metadata(dest)
        .map_err(|error| {
            format!(
                "failed to read permissions for '{}': {error}",
                dest.display()
            )
        })?
        .permissions();
    permissions.set_mode(mode.bits());
    std_fs::set_permissions(dest, permissions).map_err(|error| {
        format!(
            "failed to set assembly file mode '{:04o}' on '{}': {error}",
            mode.bits(),
            dest.display()
        )
    })
}

#[cfg(not(unix))]
pub(super) fn apply_mode(_dest: &Path, _mode: Option<gaia_spec::FileMode>) -> Result<(), String> {
    Ok(())
}

pub(super) fn file_len(path: &Path) -> Result<u64, String> {
    std_fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|error| {
            format!(
                "failed to read assembly output '{}': {error}",
                path.display()
            )
        })
}

pub(super) fn file_sha256(path: &Path) -> Result<String, String> {
    let mut file = std_fs::File::open(path).map_err(|error| {
        format!(
            "failed to open assembly output for digest '{}': {error}",
            path.display()
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            format!(
                "failed to read assembly output for digest '{}': {error}",
                path.display()
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn normalize_path_lossy(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        normalized.push(".");
    }
    normalized
}
