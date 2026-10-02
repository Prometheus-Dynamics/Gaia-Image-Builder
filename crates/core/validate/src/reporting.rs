use gaia_spec::ResolvedBuildSpec;

use crate::ValidationDiagnostic;
use crate::diagnostics::warning;

pub(crate) fn validate_reporting(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    if !spec.reporting.outputs.summary
        && !spec.reporting.outputs.provenance
        && !spec.reporting.outputs.manifest
    {
        diagnostics.push(warning(
            "reporting_outputs_disabled",
            "all reporting outputs are disabled for this build".into(),
            Some("reporting".into()),
        ));
    }
}

/// Problems the config loader found, such as keys this Gaia does not know.
pub(crate) fn validate_config_warnings(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    for message in &spec.metadata.config_warnings {
        diagnostics.push(warning("config_unknown_key", message.clone(), None));
    }
}
