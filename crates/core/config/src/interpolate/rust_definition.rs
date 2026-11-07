//! Interpolating a Rust artifact's build fields.
use super::{interpolate_env, resolver};
use crate::env::ResolvedEnvironment;
use crate::raw::{RawArtifactDefinition, RawBuildConfig};

/// `definition` (a Rust one) with `${...}` resolved in its package, target
/// name, features, cargo environment, link mode, surface list and plugin
/// directories, and in the users of its trim.
pub(super) fn interpolate(
    mut definition: RawArtifactDefinition,
    raw: &RawBuildConfig,
    env: &ResolvedEnvironment,
) -> RawArtifactDefinition {
    let text = |value: String| resolver::interpolate_string(value, raw, env);
    if let RawArtifactDefinition::Rust {
        package,
        target_name,
        features,
        build_env,
        link,
        surface,
        trim,
        ..
    } = &mut definition
    {
        *package = text(std::mem::take(package));
        *target_name = target_name.take().map(text);
        *features = std::mem::take(features).into_iter().map(text).collect();
        *build_env = interpolate_env(std::mem::take(build_env), raw, env);
        *link = link.take().map(text);
        if let Some(surface) = surface {
            surface.exports = text(std::mem::take(&mut surface.exports));
            surface.plugin_dirs = std::mem::take(&mut surface.plugin_dirs)
                .into_iter()
                .map(text)
                .collect();
        }
        if let Some(trim) = trim {
            trim.users = std::mem::take(&mut trim.users)
                .into_iter()
                .map(text)
                .collect();
        }
    }
    definition
}
