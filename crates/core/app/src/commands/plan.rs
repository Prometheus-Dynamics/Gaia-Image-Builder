use gaia_config::{ResolveOptions, try_resolve_config_with_options};
use gaia_plan::plan_build_with_reuse_state;
use gaia_validate::validate_spec_with_providers;

use crate::AppContext;

use super::{CommandOutcome, load_reuse_state};

pub fn plan_build_command(
    context: &AppContext,
    build: &str,
    options: &ResolveOptions,
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
    let plan = plan_build_with_reuse_state(
        &spec,
        &context.source_catalog,
        &context.artifact_catalog,
        &context.image_catalog,
        reuse_state.as_ref(),
    );
    let diagnostics = plan.validate();
    CommandOutcome::Planned {
        spec,
        plan,
        diagnostics,
    }
}
