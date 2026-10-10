//! Checks of `--rebuild` and `--rebuild-package` against the build they name.

use gaia_plan::{ExecutionPlan, RebuildRequest};
use gaia_spec::{ImageProviderKind, ResolvedBuildSpec};

/// Errors when a `--rebuild` names no operation of the full `plan`, or when
/// `--rebuild-package` is used with an image that is not Buildroot.
pub(crate) fn check_rebuild_request(
    spec: &ResolvedBuildSpec,
    plan: &ExecutionPlan,
    rebuild: &RebuildRequest,
) -> Result<(), String> {
    rebuild.check(plan)?;
    let provider = spec.image.provider_kind();
    if !rebuild.packages.is_empty() && provider != ImageProviderKind::Buildroot {
        return Err(format!(
            "--rebuild-package applies to Buildroot images, and build '{}' uses {provider:?}",
            spec.identity.display_name
        ));
    }
    Ok(())
}
