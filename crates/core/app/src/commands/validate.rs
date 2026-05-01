use gaia_config::{ResolveOptions, try_resolve_config_with_options};
use gaia_validate::validate_spec_with_providers;

use crate::AppContext;

use super::CommandOutcome;

pub fn validate_build_command(
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
    CommandOutcome::Validated { spec, validation }
}
