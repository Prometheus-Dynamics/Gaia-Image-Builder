//! `gaia prefetch` for Buildroot: configures the tree the way a run does,
//! then `make source` downloads the source of every enabled package into the
//! download cache. Nothing is extracted, built or installed, and no output
//! tree state is recorded beyond the configuration itself.
use super::*;
use std::collections::BTreeMap;

/// Configures `output_dir` as a run would, then runs `make source` with the
/// run's environment. Returns the step messages and a summary of what the
/// download cache gained.
pub(crate) fn prefetch_buildroot_sources(
    request: BuildrootRunRequest<'_>,
) -> Result<Vec<String>, ImageProviderError> {
    let BuildrootRunRequest {
        spec,
        image,
        buildroot_dir,
        output_dir,
        command: command_context,
    } = request;
    fs::create_dir_all(output_dir).map_err(|error| {
        ImageProviderError::backend_command(format!(
            "failed to create buildroot output dir '{}': {error}",
            output_dir.display()
        ))
    })?;
    let configured = configure_tree(
        spec,
        image,
        buildroot_dir,
        output_dir,
        &command_context,
        false,
    )?;
    let mut messages = configured.messages.clone();
    let download_dir = match configured_download_dir(spec, command_context.policy)? {
        Some(dir) => dir,
        None => default_cache_dir(spec, DEFAULT_BUILDROOT_DOWNLOAD_DIR, "buildroot download")?,
    };
    let before = download_cache_files(&download_dir);
    let mut command = Command::new("make");
    command
        .arg(format!("O={}", output_dir.display()))
        .arg("source")
        .current_dir(buildroot_dir);
    apply_buildroot_policy_env(&mut command, spec, command_context.policy)?;
    if let Some(external_tree) = configured.br2_external.as_deref() {
        command.env("BR2_EXTERNAL", external_tree);
    }
    append_make_jobs(&mut command, command_context.policy.local_jobs);
    messages.extend(run_command(
        command,
        "buildroot make source",
        command_context.execution,
        command_context.policy,
        command_context.log_sink.clone(),
        command_context.cancel_check.clone(),
    )?);
    let after = download_cache_files(&download_dir);
    messages.push(prefetch_summary(&download_dir, &before, &after));
    Ok(messages)
}

/// Files in the download cache, by path, with their sizes.
fn download_cache_files(dir: &Path) -> BTreeMap<PathBuf, u64> {
    let mut files = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file()
                && let Ok(metadata) = entry.metadata()
            {
                files.insert(path, metadata.len());
            }
        }
    }
    files
}

/// `N new file(s) (size) downloaded; the cache holds M file(s) (size)`.
fn prefetch_summary(
    dir: &Path,
    before: &BTreeMap<PathBuf, u64>,
    after: &BTreeMap<PathBuf, u64>,
) -> String {
    let added = after
        .iter()
        .filter(|(path, _)| !before.contains_key(*path))
        .map(|(_, size)| *size)
        .collect::<Vec<_>>();
    let added_bytes: u64 = added.iter().sum();
    let total_bytes: u64 = after.values().sum();
    format!(
        "prefetch: {} new source file(s) ({}) downloaded; download cache '{}' now holds {} file(s), {}",
        added.len(),
        crate::build_reports::human_bytes(added_bytes),
        dir.display(),
        after.len(),
        crate::build_reports::human_bytes(total_bytes)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_counts_only_files_that_appeared() {
        let dir = Path::new("/cache/dl");
        let before = BTreeMap::from([(PathBuf::from("/cache/dl/a.tar.gz"), 1024_u64)]);
        let after = BTreeMap::from([
            (PathBuf::from("/cache/dl/a.tar.gz"), 1024_u64),
            (PathBuf::from("/cache/dl/b/b.tar.xz"), 2048_u64),
        ]);
        let text = prefetch_summary(dir, &before, &after);
        assert!(text.starts_with("prefetch: 1 new source file(s)"), "{text}");
        assert!(text.contains("now holds 2 file(s)"), "{text}");
    }

    #[test]
    fn download_cache_files_walks_subdirectories() {
        let scratch = gaia_testkit::TempDir::new("gaia-prefetch-walk");
        let dir = scratch.join("dl");
        fs::create_dir_all(dir.join("pkg")).expect("pkg dir");
        fs::write(dir.join("top.tar"), b"123").expect("top");
        fs::write(dir.join("pkg/inner.tar"), b"4567").expect("inner");
        let files = download_cache_files(&dir);
        assert_eq!(files.len(), 2);
        assert_eq!(files.values().sum::<u64>(), 7);
    }
}
