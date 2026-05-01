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
