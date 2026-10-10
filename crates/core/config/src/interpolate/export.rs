//! `export_dir` values (`[image.output]` and `[workspace]`) take the same
//! `${...}` interpolation as the other path settings.
use crate::env::ResolvedEnvironment;
use crate::raw::RawBuildConfig;

use super::resolver;

pub(super) fn interpolate_export_dirs(
    snapshot: &RawBuildConfig,
    interpolated: &mut RawBuildConfig,
    env: &ResolvedEnvironment,
) {
    interpolated.workspace.export_dir = snapshot
        .workspace
        .export_dir
        .clone()
        .map(|value| resolver::interpolate_string(value, snapshot, env));
    interpolated.image.output.export_dir = snapshot
        .image
        .output
        .export_dir
        .clone()
        .map(|value| resolver::interpolate_string(value, snapshot, env));
}
