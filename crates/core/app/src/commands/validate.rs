use gaia_config::{ResolveOptions, VariantBuild};
use gaia_spec::ResolvedBuildSpec;
use gaia_validate::{
    DiagnosticSeverity, ValidationDiagnostic, ValidationReport, validate_spec_with_providers,
};

use crate::AppContext;

use super::CommandOutcome;
use super::variants::{VariantSource, prepare_variants, selected_variants, variant_id};

/// Validates the build. A build with `[[variants]]` is validated as every
/// variant (or the ones `--variant` names), each resolved the way `run` and
/// `preview` resolve it, with every diagnostic prefixed by its variant.
pub fn validate_build_command(
    context: &AppContext,
    build: &str,
    options: &ResolveOptions,
    variants: &[String],
) -> CommandOutcome {
    let loaded = match VariantBuild::load(build, options) {
        Ok(loaded) => loaded,
        Err(error) => {
            return CommandOutcome::Failed {
                message: format!("{error}\nvalidate: FAILED (the build config did not resolve)"),
            };
        }
    };
    let specs = match loaded.resolve() {
        Ok(specs) => specs,
        Err(error) => {
            return CommandOutcome::Failed {
                message: format!("{error}\nvalidate: FAILED (the build config did not resolve)"),
            };
        }
    };
    let has_variants = specs[0].selection.variant.is_some();
    if !has_variants {
        if !variants.is_empty() {
            return CommandOutcome::Failed {
                message: format!(
                    "--variant needs a build that declares [[variants]]; '{build}' declares none"
                ),
            };
        }
        let spec = specs.into_iter().next().expect("one spec");
        let validation = validate_one(context, &spec);
        return CommandOutcome::Validated { spec, validation };
    }
    let selected = match selected_variants(&specs, variants) {
        Ok(selected) => selected,
        Err(message) => return CommandOutcome::Failed { message },
    };
    let mut validation = ValidationReport::default();
    for index in selected {
        let spec = &specs[index];
        let id = variant_id(spec).to_string();
        let report = validate_one(context, spec);
        merge_prefixed(&mut validation, report, &id);
    }
    let spec = specs[0].clone();
    // What `run` and `preview` check next across all variants: which
    // operations are shared, sources the base needs that a variant changes,
    // and colliding outputs once paths are scoped.
    if validation.errors.is_empty()
        && let Err(message) = prepare_variants(context, VariantSource { build: &loaded }, specs)
    {
        // Printed from the diagnostics, like every other error.
        validation.diagnostics.push(ValidationDiagnostic {
            severity: DiagnosticSeverity::Error,
            code: "variants_invalid",
            message: message.clone(),
            location: Some("variants".into()),
        });
        validation.errors.push(message);
    }
    CommandOutcome::Validated { spec, validation }
}

fn validate_one(context: &AppContext, spec: &ResolvedBuildSpec) -> ValidationReport {
    validate_spec_with_providers(
        spec,
        &context.source_catalog,
        &context.artifact_catalog,
        &context.image_catalog,
    )
}

/// Adds `report` to `into`, each message prefixed with `variant <id>: `.
pub(crate) fn merge_prefixed(into: &mut ValidationReport, report: ValidationReport, id: &str) {
    let prefix = |message: &str| format!("variant {id}: {message}");
    into.errors
        .extend(report.errors.iter().map(|message| prefix(message)));
    into.warnings
        .extend(report.warnings.iter().map(|message| prefix(message)));
    into.diagnostics
        .extend(report.diagnostics.into_iter().map(|mut diagnostic| {
            diagnostic.message = prefix(&diagnostic.message);
            diagnostic
        }));
}
