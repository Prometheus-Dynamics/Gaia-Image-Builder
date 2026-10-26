use std::collections::HashSet;

use gaia_spec::{ResolvedBuildSpec, SourceDefinition};

use crate::ValidationDiagnostic;
use crate::diagnostics::{error, warning};
use crate::workspace::resolve_workspace_path;

pub(crate) fn validate_sources(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) -> HashSet<String> {
    let mut source_ids = HashSet::new();
    for source in &spec.sources {
        if !source.id.is_valid() {
            diagnostics.push(error(
                "source_id_empty",
                "source id cannot be empty".into(),
                Some("source".into()),
            ));
        }
        if !source_ids.insert(source.id.as_str().to_string()) {
            diagnostics.push(error(
                "duplicate_source_id",
                format!("duplicate source id '{}'", source.id.as_str()),
                Some(format!("source:{}", source.id.as_str())),
            ));
        }

        match &source.definition {
            SourceDefinition::Git(git) => {
                let selector_count = [git.branch.as_ref(), git.tag.as_ref(), git.rev.as_ref()]
                    .into_iter()
                    .flatten()
                    .count();
                if git.repo.trim().is_empty() {
                    diagnostics.push(error(
                        "git_repo_empty",
                        format!("git source '{}' has an empty repo", source.id.as_str()),
                        Some(format!("source:{}", source.id.as_str())),
                    ));
                }
                if selector_count > 1 {
                    diagnostics.push(error(
                        "git_selector_conflict",
                        format!(
                            "git source '{}' sets more than one selector among branch/tag/rev",
                            source.id.as_str()
                        ),
                        Some(format!("source:{}", source.id.as_str())),
                    ));
                }
            }
            SourceDefinition::Path(path) => {
                if path.path.trim().is_empty() {
                    diagnostics.push(error(
                        "path_source_empty",
                        format!("path source '{}' has an empty path", source.id.as_str()),
                        Some(format!("source:{}", source.id.as_str())),
                    ));
                } else if let Err(message) = resolve_workspace_path(spec, &path.path) {
                    diagnostics.push(error(
                        "path_source_invalid",
                        format!(
                            "path source '{}' has an invalid path: {message}",
                            source.id.as_str()
                        ),
                        Some(format!("source:{}", source.id.as_str())),
                    ));
                }
            }
            SourceDefinition::Archive(archive) if archive.is_url() => {
                validate_archive_url(source.id.as_str(), archive, diagnostics);
            }
            SourceDefinition::Archive(archive) => {
                if archive.sha256.is_some() {
                    diagnostics.push(error(
                        "archive_sha256_without_url",
                        format!(
                            "archive source '{}' sets sha256 without url; sha256 checks a downloaded archive",
                            source.id.as_str()
                        ),
                        Some(format!("source:{}", source.id.as_str())),
                    ));
                }
                if archive.path.trim().is_empty() {
                    diagnostics.push(error(
                        "archive_source_empty",
                        format!("archive source '{}' has an empty path", source.id.as_str()),
                        Some(format!("source:{}", source.id.as_str())),
                    ));
                } else if let Err(message) = resolve_workspace_path(spec, &archive.path) {
                    diagnostics.push(error(
                        "archive_source_invalid",
                        format!(
                            "archive source '{}' has an invalid path: {message}",
                            source.id.as_str()
                        ),
                        Some(format!("source:{}", source.id.as_str())),
                    ));
                }
            }
            SourceDefinition::Download(download) => {
                if download.url.trim().is_empty() {
                    diagnostics.push(error(
                        "download_url_empty",
                        format!("download source '{}' has an empty url", source.id.as_str()),
                        Some(format!("source:{}", source.id.as_str())),
                    ));
                }
                if download.output_path.trim().is_empty() {
                    diagnostics.push(error(
                        "download_output_empty",
                        format!(
                            "download source '{}' has an empty output path",
                            source.id.as_str()
                        ),
                        Some(format!("source:{}", source.id.as_str())),
                    ));
                }
            }
        }
    }
    validate_git_source_refs(spec, diagnostics);
    validate_git_lockfile(spec, diagnostics);
    source_ids
}

/// An archive source that is downloaded: `url` with its `sha256`, and no
/// `path`. The checksum is what the archive is verified against before it is
/// extracted, so it is required.
fn validate_archive_url(
    id: &str,
    archive: &gaia_spec::ArchiveSourceSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let location = Some(format!("source:{id}"));
    if !archive.path.trim().is_empty() {
        diagnostics.push(error(
            "archive_path_and_url",
            format!("archive source '{id}' sets both path and url; choose one"),
            location.clone(),
        ));
    }
    let url = archive.url.as_deref().unwrap_or_default();
    if url.trim().is_empty() {
        diagnostics.push(error(
            "archive_url_empty",
            format!("archive source '{id}' has an empty url"),
            location.clone(),
        ));
    } else if !["https://", "http://", "file://"]
        .iter()
        .any(|scheme| url.starts_with(scheme))
    {
        diagnostics.push(error(
            "archive_url_invalid",
            format!(
                "archive source '{id}' url '{url}' must start with https://, http:// or file://"
            ),
            location.clone(),
        ));
    }
    match archive.sha256.as_deref() {
        None => diagnostics.push(error(
            "archive_sha256_missing",
            format!(
                "archive source '{id}' downloads from url and needs sha256 (the archive is verified before it is extracted)"
            ),
            location,
        )),
        Some(sha) if !is_sha256_hex(sha) => diagnostics.push(error(
            "archive_sha256_invalid",
            format!("archive source '{id}' sha256 must be 64 hexadecimal digits"),
            location,
        )),
        Some(_) => {}
    }
}

/// A SHA-256 written as 64 hexadecimal digits (either case).
pub(crate) fn is_sha256_hex(sha: &str) -> bool {
    sha.len() == 64 && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Normalizes a repository URL for comparison (`.git` suffix, trailing `/`).
fn normalized_repo(repo: &str) -> String {
    let trimmed = repo.trim().trim_end_matches('/');
    trimmed
        .strip_suffix(".git")
        .unwrap_or(trimmed)
        .to_ascii_lowercase()
}

/// Warns when two sources check out the same repository at different refs,
/// which usually means one of them was not updated along with the other.
fn validate_git_source_refs(spec: &ResolvedBuildSpec, diagnostics: &mut Vec<ValidationDiagnostic>) {
    let git_sources = spec
        .sources
        .iter()
        .filter_map(|source| match &source.definition {
            SourceDefinition::Git(git) => Some((source.id.as_str(), git)),
            _ => None,
        })
        .collect::<Vec<_>>();
    for (index, (id, git)) in git_sources.iter().enumerate() {
        for (other_id, other) in &git_sources[index + 1..] {
            if normalized_repo(&git.repo) == normalized_repo(&other.repo)
                && git.ref_selector() != other.ref_selector()
            {
                diagnostics.push(warning(
                    "git_source_ref_divergence",
                    format!(
                        "git sources '{id}' ({}) and '{other_id}' ({}) use the same repo '{}' at different refs",
                        git.ref_selector(),
                        other.ref_selector(),
                        gaia_spec::redact_url_userinfo(&git.repo)
                    ),
                    Some(format!("source:{other_id}")),
                ));
            }
        }
    }
}

/// Reports lockfile entries that no longer match the configured source, and
/// lockfiles that cannot be read (they would otherwise be silently ignored).
fn validate_git_lockfile(spec: &ResolvedBuildSpec, diagnostics: &mut Vec<ValidationDiagnostic>) {
    use gaia_config::lockfile::{GitLockStatus, GitLockfile, lockfile_path};

    let Some(path) = lockfile_path(spec) else {
        return;
    };
    let lock = match GitLockfile::load(&path) {
        Ok(Some(lock)) => lock,
        Ok(None) => return,
        Err(message) => {
            diagnostics.push(error(
                "git_lockfile_invalid",
                format!("{message}; fix it or regenerate it with `gaia lock`"),
                Some(path.display().to_string()),
            ));
            return;
        }
    };
    for source in &spec.sources {
        let SourceDefinition::Git(git) = &source.definition else {
            continue;
        };
        if let GitLockStatus::Stale(entry) = lock.status(source.id.as_str(), git) {
            diagnostics.push(warning(
                "git_lock_stale",
                format!(
                    "lockfile '{}' pins git source '{}' for {} at {}, but the config now uses {} at {}; the lock entry is ignored until `gaia lock --update {}`",
                    path.display(),
                    source.id.as_str(),
                    entry.repo,
                    entry.reference,
                    gaia_spec::redact_url_userinfo(&git.repo),
                    git.ref_selector(),
                    source.id.as_str()
                ),
                Some(format!("source:{}", source.id.as_str())),
            ));
        }
    }
}
