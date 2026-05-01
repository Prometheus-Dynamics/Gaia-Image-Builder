use gaia_config::{ResolveOptions, try_resolve_config_with_options};

use super::CommandOutcome;

pub fn resolve_build_command(build: &str, options: &ResolveOptions) -> CommandOutcome {
    let spec = match try_resolve_config_with_options(build, options) {
        Ok(spec) => spec,
        Err(error) => {
            return CommandOutcome::Failed {
                message: error.to_string(),
            };
        }
    };
    CommandOutcome::Resolved { spec }
}
