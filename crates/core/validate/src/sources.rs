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
            SourceDefinition::Archive(archive) => {
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
                        git.repo
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
                    git.repo,
                    git.ref_selector(),
                    source.id.as_str()
                ),
                Some(format!("source:{}", source.id.as_str())),
            ));
        }
    }
}
