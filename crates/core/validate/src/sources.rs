use std::collections::HashSet;

use gaia_spec::{ResolvedBuildSpec, SourceDefinition};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;
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
    source_ids
}
