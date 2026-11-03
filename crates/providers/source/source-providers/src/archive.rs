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
        // A verified archive's tree is extracted once per workspace: a cached
        // tree is copied in instead.
        let cache_dir = archive_cache::extract_cache_dir(&execution.workspace_root, archive);
        let extract_dir = match archive.extract_to.as_deref() {
            Some(dir) => {
                let extract_dir = materialized_dir.join(dir);
                fs::create_dir_all(&extract_dir).map_err(|error| {
                    SourceProviderError::new(
                        SourceProviderErrorKind::RuntimeState,
                        format!(
                            "failed to create extract_to dir '{}': {error}",
                            extract_dir.display()
                        ),
                    )
                })?;
                extract_dir
            }
            None => materialized_dir.clone(),
        };
        let cache_hit = match cache_dir.as_deref() {
            Some(cached) if cached.is_dir() => {
                archive_cache::copy_tree(cached, &extract_dir)?;
                true
            }
            _ => false,
        };
        if !cache_hit {
            extract_archive(
                &archive_file,
                archive,
                &extract_dir,
                &execution,
                SourceCommandPolicy::new(spec, (&spec.policy.providers.archive).into()),
                log_sink,
                cancel_check,
            )?;
            if let Some(cached) = cache_dir.as_deref() {
                archive_cache::store(&extract_dir, cached);
            }
        }
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
            &(format!(
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
            ) + &members_marker(archive)),
        )?;
        Ok(vec![format!(
            "archive source '{}' {} '{}'",
            source.id.as_str(),
            if cache_hit {
                "restored from the extract cache into"
            } else {
                "extracted into"
            },
            materialized_dir.display()
        )])
    }
}

/// Extracts the archive file `archive_path` into `output_dir`, dropping the
/// source's `strip_components` leading path components and checking its
/// entries under its link rules (`allow_absolute_symlinks`).
pub(crate) fn extract_archive(
    archive_path: &Path,
    archive: &gaia_spec::ArchiveSourceSpec,
    output_dir: &Path,
    execution: &SourceExecutionContext,
    policy: SourceCommandPolicy,
    log_sink: Option<ProcessLogSink>,
    cancel_check: Option<ProcessCancelCheck>,
) -> Result<(), SourceProviderError> {
    gaia_process::validate_tar_archive_entries_with(
        archive_path,
        usize::try_from(archive.strip_components).unwrap_or(usize::MAX),
        archive.allow_absolute_symlinks,
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
    if archive.strip_components > 0 {
        command.arg(format!("--strip-components={}", archive.strip_components));
    }
    // Only the members that match (GNU tar wildcards over the paths in the
    // archive); a pattern that matches nothing fails the extract.
    if !archive.members.is_empty() {
        command.arg("--wildcards").arg("--").args(&archive.members);
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

/// The source state's `members=` lines, one per pattern; nothing when the
/// whole archive is extracted, so existing states keep their text.
fn members_marker(archive: &gaia_spec::ArchiveSourceSpec) -> String {
    let mut marker: String = archive
        .members
        .iter()
        .map(|member| format!("members={member}\n"))
        .collect();
    if let Some(dir) = &archive.extract_to {
        marker.push_str(&format!("extract_to={dir}\n"));
    }
    marker
}
