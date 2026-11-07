//! Interpolating a command artifact's fields (`kind = "command"`).
use super::{interpolate_env, resolver};
use crate::env::ResolvedEnvironment;
use crate::raw::{RawArtifactDefinition, RawBuildConfig};

/// `definition` (a command one) with `${...}` resolved in its argv, its
/// environment and its collect paths.
pub(super) fn interpolate(
    mut definition: RawArtifactDefinition,
    raw: &RawBuildConfig,
    env: &ResolvedEnvironment,
) -> RawArtifactDefinition {
    let text = |value: String| resolver::interpolate_string(value, raw, env);
    if let RawArtifactDefinition::Command {
        command,
        build_env,
        collect,
        collect_exclude,
    } = &mut definition
    {
        *command = std::mem::take(command).into_iter().map(text).collect();
        *build_env = interpolate_env(std::mem::take(build_env), raw, env);
        *collect = collect.take().map(text);
        *collect_exclude = std::mem::take(collect_exclude)
            .into_iter()
            .map(text)
            .collect();
    }
    definition
}

/// Each string field of a command artifact with the location the scanner
/// reports an unresolved token at.
pub(super) fn fields(
    artifact_id: &str,
    definition: &RawArtifactDefinition,
) -> Vec<(String, String)> {
    let RawArtifactDefinition::Command {
        command,
        build_env,
        collect,
        collect_exclude,
    } = definition
    else {
        return Vec::new();
    };
    let prefix = format!("artifacts.{artifact_id}.command");
    let mut fields = Vec::new();
    for (index, value) in command.iter().enumerate() {
        fields.push((format!("{prefix}.command.{index}"), value.clone()));
    }
    for (index, (key, value)) in build_env.iter().enumerate() {
        fields.push((format!("{prefix}.build_env.{index}.key"), key.clone()));
        fields.push((format!("{prefix}.build_env.{index}.value"), value.clone()));
    }
    if let Some(collect) = collect {
        fields.push((format!("{prefix}.collect"), collect.clone()));
    }
    for (index, value) in collect_exclude.iter().enumerate() {
        fields.push((format!("{prefix}.collect_exclude.{index}"), value.clone()));
    }
    fields
}
