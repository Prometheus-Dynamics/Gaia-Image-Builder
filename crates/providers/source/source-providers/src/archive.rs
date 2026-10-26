use super::*;
use crate::download::cached_verified_download;

impl SourceProvider for ArchiveSourceProvider {
    fn id(&self) -> &'static str {
        "source.archive"
    }

    fn kind(&self) -> SourceProviderKind {
        SourceProviderKind::Archive
    }

    fn execute_source(
        &self,
        spec: &ResolvedBuildSpec,
        source: &SourceSpec,
        log_sink: Option<ProcessLogSink>,
        cancel_check: Option<ProcessCancelCheck>,
    ) -> Result<Vec<String>, SourceProviderError> {
        let SourceDefinition::Archive(archive) = &source.definition else {
            return Err(SourceProviderError::new(
                SourceProviderErrorKind::RuntimeState,
                format!("source '{}' was not an archive source", source.id.as_str()),
            ));
        };
        let materialized_dir = materialized_dir(spec, source);
        prepare_materialized_dir(&materialized_dir)?;
        let execution = execution_context(spec);

        // A downloaded archive is verified in the download cache before it is
        // extracted; a `path` archive is read where it is.
        let archive_file = match (&archive.url, &archive.sha256) {
            (Some(url), Some(sha)) => cached_verified_download(
                spec,
                &execution,
                url,
                sha,
                log_sink.clone(),
                cancel_check.clone(),
            )?,
            (Some(_), None) => {
                return Err(SourceProviderError::new(
                    SourceProviderErrorKind::PolicyBlocked,
                    format!(
                        "archive source '{}' downloads from url without sha256",
                        source.id.as_str()
                    ),
                ));
            }
            (None, _) => resolve_workspace_path(&spec.workspace, &archive.path)?,
        };
        extract_archive(
            &archive_file,
            archive.strip_components,
            &materialized_dir,
            &execution,
            SourceCommandPolicy {
                attempts: spec.policy.providers.archive.retry_attempts.max(1),
                retry_backoff_ms: spec.policy.providers.archive.retry_backoff_ms,
                retry_backoff_strategy: spec.policy.providers.archive.retry_backoff_strategy,
                timeout_seconds: spec.policy.providers.archive.timeout_seconds.max(1),
                output_retention: process_output_retention(spec),
            },
            log_sink,
            cancel_check,
        )?;
        let origin = match &archive.url {
            Some(url) => format!(
                "archive_url={}\narchive_sha256={}",
                redact_url_userinfo(url),
                archive
                    .sha256
                    .as_deref()
                    .unwrap_or_default()
                    .to_ascii_lowercase()
            ),
            None => format!(
                "archive={}\narchive_sha256={}",
                archive.path,
                sha256_or_placeholder(&archive_file)
            ),
        };
        write_source_marker(
            spec,
            self.id(),
            source,
            &materialized_dir,
            &format!(
                "{origin}\nchecksum_policy={}\nchecksum_source={}\nextracted_tree_digest={}\nstrip_components={}\n",
                if archive.url.is_some() {
                    "verified"
                } else {
                    "observed-only"
                },
                if archive.url.is_some() {
                    "config"
                } else {
                    "archive-file"
                },
                tree_digest(&materialized_dir, &["source.txt", ".gaia-source-state.txt"]),
                archive.strip_components
            ),
        )?;
        Ok(vec![format!(
            "archive source '{}' extracted into '{}'",
            source.id.as_str(),
            materialized_dir.display()
        )])
    }
}

/// Extracts the archive file `archive_path` into `output_dir`, dropping
/// `strip_components` leading path components.
pub(crate) fn extract_archive(
    archive_path: &Path,
    strip_components: u32,
    output_dir: &Path,
    execution: &SourceExecutionContext,
    policy: SourceCommandPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(), SourceProviderError> {
    gaia_process::validate_tar_archive_entries(
        archive_path,
        usize::try_from(strip_components).unwrap_or(usize::MAX),
        Duration::from_secs(policy.timeout_seconds.max(1)),
        "validate archive source entries",
        log_sink.clone(),
        cancel_check.clone(),
    )
    .map_err(|error| {
        SourceProviderError::new(SourceProviderErrorKind::PolicyBlocked, error.message)
    })?;
    let mut command = Command::new("tar");
    command
        .arg("-xf")
        .arg(archive_path)
        .arg("--no-same-owner")
        .arg("--no-same-permissions")
        .arg("--delay-directory-restore")
        .arg("-C")
        .arg(output_dir);
    if strip_components > 0 {
        command.arg(format!("--strip-components={strip_components}"));
    }
    run_command_with_policy(
        command,
        execution,
        "extract archive source",
        policy,
        log_sink,
        cancel_check,
    )
}
