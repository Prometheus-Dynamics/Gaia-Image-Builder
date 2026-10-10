use gaia_spec::ResolvedBuildSpec;

use crate::{DiagnosticSeverity, ValidationReport};

pub fn validate_spec(spec: &ResolvedBuildSpec) -> ValidationReport {
    let mut diagnostics = Vec::new();

    crate::reporting::validate_config_warnings(spec, &mut diagnostics);
    crate::inputs::validate_inputs(spec, &mut diagnostics);
    let source_ids = crate::sources::validate_sources(spec, &mut diagnostics);
    let artifact_ids = crate::artifacts::validate_artifacts(spec, &source_ids, &mut diagnostics);
    crate::install_stage::validate_install_and_stage(spec, &artifact_ids, &mut diagnostics);
    crate::checkpoints::validate_checkpoints(spec, &mut diagnostics);
    crate::image::validate_image_contract(spec, &mut diagnostics);
    crate::reporting::validate_reporting(spec, &mut diagnostics);
    crate::buildroot_policy::validate_buildroot_policy(spec, &mut diagnostics);
    crate::java_policy::validate_java_policy(spec, &mut diagnostics);

    let warnings = diagnostics
        .iter()
        .filter(|d| d.severity == DiagnosticSeverity::Warning)
        .map(|d| d.message.clone())
        .collect();
    let errors = diagnostics
        .iter()
        .filter(|d| d.severity == DiagnosticSeverity::Error)
        .map(|d| d.message.clone())
        .collect();

    ValidationReport {
        warnings,
        errors,
        diagnostics,
    }
}
