use crate::ValidationDiagnostic;
use crate::diagnostics::warning;
use gaia_spec::ResolvedBuildSpec;

/// Warns about install and stage entries an explicit `[image.feed]` list
/// leaves out. A feed list that is set feeds only the ids it names (lists
/// from every import merge), so an entry a later layer declares without
/// adding it to the list is built and staged but never reaches the rootfs.
/// The default feed (no list) takes every entry, so this only fires when a
/// list was written. Not for `kind = "none"`, which has no feed.
pub(crate) fn validate_image_feed_unfed(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let feed = &spec.image.feed;
    let domains = [
        (
            "image.feed.install_entries",
            "install entry",
            unfed(
                spec.install.entries.iter().map(|entry| entry.id.as_str()),
                feed.install_entries.iter().map(|id| id.as_str()),
            ),
        ),
        (
            "image.feed.stage_files",
            "stage file",
            unfed(
                spec.stage.files.iter().map(|file| file.id.as_str()),
                feed.stage_files.iter().map(|id| id.as_str()),
            ),
        ),
        (
            "image.feed.stage_env_sets",
            "stage env-set",
            unfed(
                spec.stage.env_sets.iter().map(|set| set.id.as_str()),
                feed.stage_env_sets.iter().map(|id| id.as_str()),
            ),
        ),
        (
            "image.feed.stage_services",
            "stage service",
            unfed(
                spec.stage
                    .services
                    .iter()
                    .map(|service| service.id.as_str()),
                feed.stage_services.iter().map(|id| id.as_str()),
            ),
        ),
    ];
    for (path, what, ids) in domains {
        if ids.is_empty() {
            continue;
        }
        let names = ids
            .iter()
            .map(|id| format!("'{id}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let (plural, verb, them) = if ids.len() == 1 {
            ("", "is", "it")
        } else {
            ("s", "are", "them")
        };
        diagnostics.push(warning(
            "image_feed_unfed_entry",
            format!(
                "{what}{plural} {names} {verb} declared but not in {path}, so the image will not \
                 contain {them}: a feed list feeds only the ids it names; add {them} to {path} in \
                 the layer that declares {them}"
            ),
            Some(path.to_string()),
        ));
    }
}

fn unfed<'a>(
    declared: impl Iterator<Item = &'a str>,
    fed: impl Iterator<Item = &'a str>,
) -> Vec<&'a str> {
    let fed = fed.collect::<Vec<_>>();
    declared.filter(|id| !fed.contains(id)).collect()
}
