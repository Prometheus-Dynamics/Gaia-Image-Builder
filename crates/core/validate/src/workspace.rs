use gaia_spec::ResolvedBuildSpec;
use std::path::PathBuf;

pub(crate) fn resolve_workspace_path(
    spec: &ResolvedBuildSpec,
    raw: &str,
) -> Result<PathBuf, String> {
    gaia_spec::resolve_workspace_path(&spec.workspace, raw).map_err(|error| error.to_string())
}
