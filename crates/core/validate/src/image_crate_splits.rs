//! `[[image.checks.crate_splits]]` and `[[image.checks.unit_restarts]]`:
//! each entry names what it allows and says why.

use gaia_spec::ResolvedBuildSpec;

use crate::ValidationDiagnostic;
use crate::diagnostics::error;

pub(crate) fn validate_crate_splits(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    for split in &spec.image.checks.crate_splits {
        let path = Some("image.checks.crate_splits".to_string());
        if split.crate_name.trim().is_empty() {
            diagnostics.push(error(
                "image_crate_split_crate_missing",
                "an image.checks.crate_splits entry names no crate".into(),
                path,
            ));
        } else if split.reason.trim().is_empty() {
            diagnostics.push(error(
                "image_crate_split_reason_missing",
                format!(
                    "image.checks.crate_splits entry '{}' gives no reason: say why the image may hold it twice",
                    split.crate_name
                ),
                path,
            ));
        }
    }
}

pub(crate) fn validate_unit_restarts(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    for allow in &spec.image.checks.unit_restarts {
        let path = Some("image.checks.unit_restarts".to_string());
        if allow.unit.trim().is_empty() {
            diagnostics.push(error(
                "image_unit_restart_unit_missing",
                "an image.checks.unit_restarts entry names no unit".into(),
                path,
            ));
        } else if allow.reason.trim().is_empty() {
            diagnostics.push(error(
                "image_unit_restart_reason_missing",
                format!(
                    "image.checks.unit_restarts entry '{}' gives no reason: say why it may stop restarting or restart slowly",
                    allow.unit
                ),
                path,
            ));
        }
    }
}
