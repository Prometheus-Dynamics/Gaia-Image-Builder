use super::*;

impl SourceProvider for DownloadSourceProvider {
    fn id(&self) -> &'static str {
        "source.download"
    }

    fn kind(&self) -> SourceProviderKind {
        SourceProviderKind::Download
    }

    fn execute_source(
        &self,
        spec: &ResolvedBuildSpec,
        source: &SourceSpec,
        log_sink: Option<ProcessLogSink>,
        cancel_check: Option<ProcessCancelCheck>,
    ) -> Result<Vec<String>, SourceProviderError> {
        let SourceDefinition::Download(download) = &source.definition else {
            return Err(SourceProviderError::new(
                SourceProviderErrorKind::RuntimeState,
                format!("source '{}' was not a download source", source.id.as_str()),
            ));
        };
        let materialized_dir = materialized_dir(spec, source);
        prepare_materialized_dir(&materialized_dir)?;
        let execution = execution_context(spec);

        let output_path = materialized_dir.join(&download.output_path);
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                SourceProviderError::backend_command(format!(
                    "failed to create download output dir '{}': {error}",
                    parent.display()
                ))
            })?;
        }
        let expected_sha = download.sha256.as_deref().map(str::to_ascii_lowercase);
        let cache_path = expected_sha
            .as_deref()
            .and_then(|sha| download_cache_path(&execution.workspace_root, sha));
        let cache_hit = cache_path.as_deref().is_some_and(|cached| {
            // Entries are only stored after verification, so a hit is copied
            // without hashing it again.
            cached.is_file() && fs::copy(cached, &output_path).is_ok()
        });
        let actual_sha = if cache_hit {
            expected_sha.clone().unwrap_or_default()
        } else {
            let mut command = Command::new("curl");
            command
                .arg("-LfsS")
                .arg(&download.url)
                .arg("-o")
                .arg(&output_path);
            run_command_with_policy(
                command,
                &execution,
                "download source contents",
                SourceCommandPolicy {
                    attempts: spec.policy.providers.download.retry_attempts.max(1),
                    retry_backoff_ms: spec.policy.providers.download.retry_backoff_ms,
                    retry_backoff_strategy: spec.policy.providers.download.retry_backoff_strategy,
                    timeout_seconds: spec.policy.providers.download.timeout_seconds.max(1),
                    output_retention: process_output_retention(spec),
                },
                log_sink,
                cancel_check,
            )?;
            // Hash once: the same digest verifies the file and goes into the
            // source state.
            let actual_sha = sha256_or_placeholder(&output_path);
            if let Some(expected_sha) = &expected_sha {
                check_sha256(&output_path, expected_sha, &actual_sha)?;
            }
            if let Some(cache_path) = &cache_path
                && let Err(error) = store_in_download_cache(&output_path, cache_path)
            {
                tracing::warn!(cache = %cache_path.display(), %error, "failed to store download in cache");
            }
            actual_sha
        };
        write_source_marker(
            spec,
            self.id(),
            source,
            &materialized_dir,
            &format!(
                "download={}\noutput={}\noutput_sha256={}\nexpected_sha256={}\nchecksum_policy={}\nchecksum_source={}\n",
                download.url,
                output_path.display(),
                actual_sha,
                download.sha256.as_deref().unwrap_or("none"),
                if download.sha256.is_some() {
                    "verified"
                } else {
                    "observed-only"
                },
                if download.sha256.is_some() {
                    "config"
                } else {
                    "downloaded-file"
                }
            ),
        )?;
        Ok(vec![format!(
            "download source '{}' {} '{}'",
            source.id.as_str(),
            if cache_hit {
                "restored from cache"
            } else {
                "fetched"
            },
            output_path.display()
        )])
    }
}

/// Workspace-relative root of the content-addressed download cache.
pub const DOWNLOAD_CACHE_DIR: &str = ".gaia/cache/downloads";

/// Cache location for a verified download with the given SHA-256:
/// `<workspace>/.gaia/cache/downloads/sha256/<sha>`. `None` when `sha` is not
/// a hex SHA-256, so malformed checksums never address the cache.
pub fn download_cache_path(workspace_root: &Path, sha: &str) -> Option<PathBuf> {
    let valid = sha.len() == 64 && sha.bytes().all(|byte| byte.is_ascii_hexdigit());
    valid.then(|| {
        workspace_root
            .join(DOWNLOAD_CACHE_DIR)
            .join("sha256")
            .join(sha.to_ascii_lowercase())
    })
}

/// Copies a verified download into the cache through a temporary file, so a
/// partially written entry is never visible under its final name.
fn store_in_download_cache(file: &Path, cache_path: &Path) -> std::io::Result<()> {
    if cache_path.is_file() {
        return Ok(());
    }
    let parent = cache_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        cache_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        std::process::id()
    ));
    fs::copy(file, &temp)?;
    fs::rename(&temp, cache_path).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}
