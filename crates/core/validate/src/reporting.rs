use gaia_spec::ResolvedBuildSpec;

use crate::ValidationDiagnostic;
use crate::diagnostics::{error, warning};

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

/// Keys this Gaia does not know. They are errors: a file using a setting
/// from a newer Gaia must fail rather than build without it.
pub(crate) fn validate_config_warnings(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    for message in &spec.metadata.config_warnings {
        diagnostics.push(error("config_unknown_key", message.clone(), None));
    }
}
