use gaia_config::{ResolveOptions, try_resolve_config_with_options};
use gaia_plan::{PlanTarget, RebuildRequest, plan_build_with_rebuilds};
use gaia_validate::validate_spec_with_providers;

use crate::AppContext;

use super::rebuild::check_rebuild_request;
use super::{CommandOutcome, load_operation_durations, load_reuse_state};

pub fn plan_build_command(
    context: &AppContext,
    build: &str,
    options: &ResolveOptions,
    targets: &[PlanTarget],
    rebuild: &RebuildRequest,
) -> CommandOutcome {
    let spec = match try_resolve_config_with_options(build, options) {
        Ok(spec) => spec,
        Err(error) => {
            return CommandOutcome::Failed {
                message: error.to_string(),
            };
        }
    };
    let validation = validate_spec_with_providers(
        &spec,
        &context.source_catalog,
        &context.artifact_catalog,
        &context.image_catalog,
    );
    if !validation.errors.is_empty() {
        return CommandOutcome::Failed {
            message: format!(
                "refusing to plan build '{}': {} validation error(s)",
                spec.identity.display_name,
                validation.errors.len()
            ),
        };
    }

    let reuse_state = load_reuse_state(&spec);
    let plan = plan_build_with_rebuilds(
        &spec,
        &context.source_catalog,
        &context.artifact_catalog,
        &context.image_catalog,
        reuse_state.as_ref(),
        rebuild,
    );
    if let Err(message) = check_rebuild_request(&spec, &plan, rebuild) {
        return CommandOutcome::Failed { message };
    }
    let plan = if targets.is_empty() {
        plan
    } else {
        match plan.restrict_to(targets) {
            Ok(plan) => plan,
            Err(message) => return CommandOutcome::Failed { message },
        }
    };
    let diagnostics = plan.validate();
    let estimate = gaia_plan::estimate_plan(&plan, &load_operation_durations(&spec));
    CommandOutcome::Planned {
        spec,
        plan,
        diagnostics,
        estimate,
    }
}
