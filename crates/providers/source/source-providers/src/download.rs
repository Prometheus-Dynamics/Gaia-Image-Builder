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
        if let Some(expected_sha) = &download.sha256 {
            verify_sha256(&output_path, expected_sha)?;
        }
        let actual_sha = sha256_or_placeholder(&output_path);
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
            "download source '{}' fetched '{}'",
            source.id.as_str(),
            output_path.display()
        )])
    }
}
