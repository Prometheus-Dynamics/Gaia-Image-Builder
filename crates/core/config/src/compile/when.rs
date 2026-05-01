use super::*;

pub(crate) fn apply_when_selection(raw: &mut RawBuildConfig) {
    let context = WhenContext {
        target: raw.target.clone(),
        profile: raw.profile.clone(),
        branch: raw.branch.clone(),
        image_kind: match raw.image.definition {
            RawImageDefinition::Buildroot { .. } => RawWhenImageKind::Buildroot,
            RawImageDefinition::StartingPoint { .. } => RawWhenImageKind::StartingPoint,
        },
    };
    let install_ids_before = raw
        .install
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<std::collections::HashSet<_>>();
    let stage_file_ids_before = raw
        .stage
        .files
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<std::collections::HashSet<_>>();
    let stage_env_ids_before = raw
        .stage
        .env_sets
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<std::collections::HashSet<_>>();
    let stage_service_ids_before = raw
        .stage
        .services
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<std::collections::HashSet<_>>();

    raw.artifacts
        .retain(|artifact| when_matches(artifact.when.as_ref(), &context));
    raw.install
        .retain(|install| when_matches(install.when.as_ref(), &context));
    raw.stage
        .files
        .retain(|file| when_matches(file.when.as_ref(), &context));
    raw.stage
        .env_sets
        .retain(|env_set| when_matches(env_set.when.as_ref(), &context));
    raw.stage
        .services
        .retain(|service| when_matches(service.when.as_ref(), &context));

    if !raw.image.feed.install_entries.is_empty() {
        let selected = raw
            .install
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        raw.image
            .feed
            .install_entries
            .retain(|id| selected.contains(id.as_str()) || !install_ids_before.contains(id));
    }
    if !raw.image.feed.stage_files.is_empty() {
        let selected = raw
            .stage
            .files
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        raw.image
            .feed
            .stage_files
            .retain(|id| selected.contains(id.as_str()) || !stage_file_ids_before.contains(id));
    }
    if !raw.image.feed.stage_env_sets.is_empty() {
        let selected = raw
            .stage
            .env_sets
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        raw.image
            .feed
            .stage_env_sets
            .retain(|id| selected.contains(id.as_str()) || !stage_env_ids_before.contains(id));
    }
    if !raw.image.feed.stage_services.is_empty() {
        let selected = raw
            .stage
            .services
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        raw.image
            .feed
            .stage_services
            .retain(|id| selected.contains(id.as_str()) || !stage_service_ids_before.contains(id));
    }
}

struct WhenContext {
    target: Option<String>,
    profile: Option<String>,
    branch: Option<String>,
    image_kind: RawWhenImageKind,
}

fn when_matches(when: Option<&RawWhenConfig>, context: &WhenContext) -> bool {
    let Some(when) = when else {
        return true;
    };

    let target_matches = when
        .target
        .as_ref()
        .is_none_or(|expected| context.target.as_deref() == Some(expected.as_str()));
    let profile_matches = when
        .profile
        .as_ref()
        .is_none_or(|expected| context.profile.as_deref() == Some(expected.as_str()));
    let branch_matches = when
        .branch
        .as_ref()
        .is_none_or(|expected| context.branch.as_deref() == Some(expected.as_str()));
    let image_kind_matches = when
        .image_kind
        .is_none_or(|expected| expected == context.image_kind);
    let all_matches = when
        .all
        .iter()
        .all(|item| when_matches(Some(item), context));
    let any_matches = if when.any.is_empty() {
        true
    } else {
        when.any
            .iter()
            .any(|item| when_matches(Some(item), context))
    };
    let not_matches = when
        .not
        .as_ref()
        .is_none_or(|item| !when_matches(Some(item.as_ref()), context));

    target_matches
        && profile_matches
        && branch_matches
        && image_kind_matches
        && all_matches
        && any_matches
        && not_matches
}
