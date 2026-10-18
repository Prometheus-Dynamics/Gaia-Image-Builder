//! The support files a defconfig names: copied into the output dir, where
//! Buildroot resolves them. Only the paths its settings and config fragments
//! mention are copied (see `materialize_defconfig_support_files`).

use super::*;

/// Entries of a Buildroot output dir a support file must never replace.
const BUILDROOT_OUTPUT_ENTRIES: &[&str] = &[
    "Makefile",
    "target",
    "build",
    "host",
    "images",
    "staging",
    "legal-info",
    "per-package",
    "graphs",
    "toolchain",
    ".config",
    ".config.old",
];

/// Copies the files a defconfig's settings name, relative to the defconfig's
/// directory, into the output dir. Buildroot resolves those names from the
/// output dir (the post-image arguments name `genimage.cfg` that way), so a
/// defconfig beside its support files finds them there.
///
/// Only the paths the defconfig and its fragments mention are copied: the
/// rest of the directory (at a project root, the whole workspace, possibly
/// including the output dir) is not. A named directory is copied whole, but
/// never into or over the output dir itself or the Buildroot entries in it.
pub(crate) fn materialize_defconfig_support_files(
    defconfig_path: &Path,
    output_dir: &Path,
    config_fragments: &[PathBuf],
) -> Result<(), ImageProviderError> {
    let Some(defconfig_dir) = defconfig_path.parent() else {
        return Ok(());
    };
    let mut texts = vec![fs::read_to_string(defconfig_path).unwrap_or_default()];
    texts.extend(
        config_fragments
            .iter()
            .filter_map(|fragment| fs::read_to_string(fragment).ok()),
    );
    let mut references = BTreeSet::new();
    for text in &texts {
        collect_setting_references(text, &mut references);
    }
    let output_root = fs::canonicalize(output_dir).unwrap_or_else(|_| output_dir.to_path_buf());
    for relative in references {
        let first = relative.components().next();
        let reserved = first.is_some_and(|component| {
            let name = component.as_os_str().to_string_lossy();
            BUILDROOT_OUTPUT_ENTRIES.contains(&name.as_ref()) || name.starts_with(".gaia-")
        });
        if reserved {
            continue;
        }
        let source = defconfig_dir.join(&relative);
        let dest = output_dir.join(&relative);
        copy_referenced_path(&source, &dest, &output_root)?;
    }
    Ok(())
}

/// The relative paths a settings text names: the whitespace-separated words
/// of each `KEY=value` line's value, quotes and a leading `./` removed.
/// Absolute paths, paths with `..`, and paths holding a `$` variable are
/// left out (they do not name a file beside the defconfig).
fn collect_setting_references(text: &str, references: &mut BTreeSet<PathBuf>) {
    for line in text.lines().map(str::trim) {
        if line.starts_with('#') {
            continue;
        }
        let Some((_, value)) = line.split_once('=') else {
            continue;
        };
        for word in value.split_whitespace() {
            let word = word.trim_matches(|c| c == '"' || c == '\'');
            if word.contains('$') {
                continue;
            }
            let path = Path::new(word.strip_prefix("./").unwrap_or(word));
            let plain = path
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)));
            if plain && path.components().next().is_some() {
                references.insert(path.to_path_buf());
            }
        }
    }
}

/// Copies one referenced file, or directory tree, from `source` to `dest`,
/// never touching the output dir (`output_root`, canonical) or anything that
/// contains it.
fn copy_referenced_path(
    source: &Path,
    dest: &Path,
    output_root: &Path,
) -> Result<(), ImageProviderError> {
    let Ok(metadata) = fs::symlink_metadata(source) else {
        return Ok(());
    };
    let canonical = fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
    if canonical.starts_with(output_root) || output_root.starts_with(&canonical) {
        return Ok(());
    }
    if metadata.is_file() {
        return copy_support_file(source, dest, &metadata);
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(dest).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to create buildroot support dir '{}': {error}",
            dest.display()
        ))
    })?;
    for entry in fs::read_dir(source).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to read buildroot support dir '{}': {error}",
            source.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to read buildroot support entry in '{}': {error}",
                source.display()
            ))
        })?;
        copy_referenced_path(&entry.path(), &dest.join(entry.file_name()), output_root)?;
    }
    Ok(())
}

/// Copies a regular file unless the copy already there has its size and
/// modification time. A copy keeps the source's mtime, which is what that
/// check compares against.
fn copy_support_file(
    source: &Path,
    dest: &Path,
    metadata: &fs::Metadata,
) -> Result<(), ImageProviderError> {
    let modified = metadata.modified().ok();
    if let Ok(current) = fs::metadata(dest)
        && current.is_file()
        && current.len() == metadata.len()
        && current.modified().ok() == modified
    {
        return Ok(());
    }
    let backend = |action: &str, error: std::io::Error| {
        ImageProviderError::backend_command(format!(
            "failed to {action} buildroot support file '{}' to '{}': {error}",
            source.display(),
            dest.display()
        ))
    };
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ImageProviderError::backend_command(format!(
                "failed to create buildroot support parent dir '{}': {error}",
                parent.display()
            ))
        })?;
    }
    fs::copy(source, dest).map_err(|error| backend("copy", error))?;
    if let Some(modified) = modified {
        fs::File::options()
            .write(true)
            .open(dest)
            .and_then(|file| file.set_modified(modified))
            .map_err(|error| backend("time-stamp", error))?;
    }
    Ok(())
}

