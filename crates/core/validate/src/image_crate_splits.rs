//! `[[image.checks.crate_splits]]`: each allowed split names one crate and
//! says why it is allowed.

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
