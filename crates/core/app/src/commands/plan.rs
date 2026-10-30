use gaia_config::{ResolveOptions, VariantBuild};
use gaia_plan::{ExecutionPlan, PlanTarget, RebuildRequest};
use gaia_spec::ResolvedBuildSpec;
use gaia_validate::validate_spec_with_providers;

use crate::AppContext;

use super::rebuild::check_rebuild_request;
use super::reuse_reasons::{explain_rebuild_reasons, recorded_for_explanation};
use super::variants::{ReuseScope, plan_variants, scoped_reuse_state};
use super::{CommandOutcome, load_operation_durations};

pub(crate) fn plan_build_command(
    context: &AppContext,
    build: &str,
    options: &ResolveOptions,
    targets: &[PlanTarget],
    rebuild: &RebuildRequest,
    variants: &[String],
) -> CommandOutcome {
    let loaded = match VariantBuild::load(build, options) {
        Ok(loaded) => loaded,
        Err(error) => {
            return CommandOutcome::Failed {
                message: error.to_string(),
            };
        }
    };
    let mut specs = match loaded.resolve() {
        Ok(specs) => specs,
        Err(error) => {
            return CommandOutcome::Failed {
                message: error.to_string(),
            };
        }
    };
    if specs[0].selection.variant.is_some() {
        return plan_variants(
            context,
            super::variants::VariantSource { build: &loaded },
            specs,
            variants,
            targets,
            rebuild,
        );
    }
    if !variants.is_empty() {
        return CommandOutcome::Failed {
            message: format!(
                "--variant needs a build that declares [[variants]]; '{build}' declares none"
            ),
        };
    }
    let spec = specs.remove(0);
    plan_resolved(context, spec, targets, rebuild, ReuseScope::Build, None)
}

/// The plan of one resolved spec (a build, or one variant of one), with the
/// reuse state `scope` names.
pub(crate) fn plan_resolved(
    context: &AppContext,
    spec: ResolvedBuildSpec,
    targets: &[PlanTarget],
    rebuild: &RebuildRequest,
    scope: ReuseScope<'_>,
    structure: Option<ExecutionPlan>,
) -> CommandOutcome {
    let validation = validate_spec_with_providers(
        &spec,
        &context.source_catalog,
        &context.artifact_catalog,
        &context.image_catalog,
    );
    if !validation.errors.is_empty() {
        return CommandOutcome::Failed {
            message: format!(
                "refusing to plan build '{}': {} validation error(s)",
                spec.identity.display_name,
                validation.errors.len()
            ),
        };
    }

    let reuse_state = scoped_reuse_state(&spec, scope);
    // Named like `run` and `preview` name them: what changed since the
    // recorded state, from the components that state recorded.
    let recorded = recorded_for_explanation(&spec, reuse_state.as_ref(), scope);
    let mut plan =
        super::run::plan_with_reuse(context, &spec, reuse_state.as_ref(), rebuild, structure);
    explain_rebuild_reasons(&spec, &mut plan, reuse_state.as_ref(), &recorded);
    if let Err(message) = check_rebuild_request(&spec, &plan, rebuild) {
        return CommandOutcome::Failed { message };
    }
    let plan = if targets.is_empty() {
        plan
    } else {
        match plan.restrict_to(targets) {
            Ok(plan) => plan,
            Err(message) => return CommandOutcome::Failed { message },
        }
    };
    let diagnostics = plan.validate();
    let estimate = gaia_plan::estimate_plan(&plan, &load_operation_durations(&spec));
    CommandOutcome::Planned {
        spec,
        plan,
        diagnostics,
        estimate,
    }
}
