use gaia_artifact_providers::ArtifactProviderCatalog;
use gaia_image_providers::{ImageProviderCatalog, ImageProviderOperation};
use gaia_source_providers::SourceProviderCatalog;
use gaia_spec::ResolvedBuildSpec;

use crate::diagnostics::error;
use crate::{DiagnosticSeverity, ValidationReport, validate_spec};

pub fn validate_spec_with_providers(
    spec: &ResolvedBuildSpec,
    source_catalog: &SourceProviderCatalog,
    artifact_catalog: &ArtifactProviderCatalog,
    image_catalog: &ImageProviderCatalog,
) -> ValidationReport {
    let mut report = validate_spec(spec);

    for source in &spec.sources {
        if let Some(provider) = source_catalog.find_for_kind(source.provider_kind()) {
            for issue in provider.validate_source(source) {
                report.diagnostics.push(error(
                    issue.code,
                    issue.message,
                    Some(format!("source:{}", source.id.as_str())),
                ));
            }
        }
    }

    for artifact in &spec.artifacts {
        if let Some(provider) = artifact_catalog.find_for_kind(artifact.provider_kind()) {
            for issue in provider.validate_artifact(artifact) {
                report.diagnostics.push(error(
                    issue.code,
                    issue.message,
                    Some(format!("artifact:{}", artifact.id.as_str())),
                ));
            }
        }
    }

    if let Some(provider) = image_catalog.find_for_kind(spec.image.provider_kind()) {
        for issue in provider.validate_image(&spec.image) {
            report
                .diagnostics
                .push(error(issue.code, issue.message, Some("image".into())));
        }
        let image_plan = provider.plan_image(&spec.image);
        if image_plan.operations.is_empty() {
            report.diagnostics.push(error(
                "image_provider_plan_empty",
                format!(
                    "image provider '{}' did not plan any image operation",
                    provider.id()
                ),
                Some("image".into()),
            ));
        }
        let prepare_count = image_plan
            .operations
            .iter()
            .filter(|operation| matches!(operation, ImageProviderOperation::Prepare))
            .count();
        if prepare_count > 1 {
            report.diagnostics.push(error(
                "image_provider_prepare_count_invalid",
                format!(
                    "image provider '{}' planned {prepare_count} prepare operations; expected at most one",
                    provider.id(),
                ),
                Some("image".into()),
            ));
        }
        let build_count = image_plan
            .operations
            .iter()
            .filter(|operation| matches!(operation, ImageProviderOperation::Build))
            .count();
        if build_count != 1 {
            report.diagnostics.push(error(
                "image_provider_build_count_invalid",
                format!(
                    "image provider '{}' must plan exactly one build operation, got {}",
                    provider.id(),
                    build_count
                ),
                Some("image".into()),
            ));
        }
    }

    report.warnings = report
        .diagnostics
        .iter()
        .filter(|d| d.severity == DiagnosticSeverity::Warning)
        .map(|d| d.message.clone())
        .collect();
    report.errors = report
        .diagnostics
        .iter()
        .filter(|d| d.severity == DiagnosticSeverity::Error)
        .map(|d| d.message.clone())
        .collect();

    report
}
