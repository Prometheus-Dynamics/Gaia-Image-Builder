mod manifest;
mod masking;
mod model;
mod output;
mod provenance;
mod rebuild;
mod selection;
mod state;
mod summary;

use gaia_exec::ExecutionOutcome;
use gaia_plan::ExecutionPlan;
use gaia_spec::ResolvedBuildSpec;
use gaia_validate::ValidationReport;

pub use manifest::{render_manifest, render_manifest_with_outcome};
pub use masking::{mask_pairs, mask_value};
pub use model::*;
pub use output::write_report_bundle;
pub use provenance::render_provenance;
pub use rebuild::render_rebuild_reasons;
pub use selection::render_selection;
pub use summary::{render_execution_failures, render_summary};

pub fn generate_report(
    spec: &ResolvedBuildSpec,
    validation: &ValidationReport,
    plan: &ExecutionPlan,
    outcome: &ExecutionOutcome,
) -> ReportBundle {
    let span = tracing::info_span!(
        "generate_report",
        build_id = %spec.identity.id.as_str(),
        build_name = %spec.identity.build_name,
        diagnostics = validation.diagnostics.len(),
        operations = plan.operations.len(),
        completed_operations = outcome.completed_operations,
        errors = outcome.errors.len(),
        cancelled = outcome.cancelled,
    );
    let _guard = span.enter();
    ReportBundle {
        summary: summary::render_summary(spec, validation, plan, outcome),
        selection: selection::render_selection(spec),
        provenance: provenance::render_provenance(spec, outcome),
        manifest: manifest::render_manifest_with_outcome(spec, plan, outcome),
        rebuild_reasons: rebuild::render_rebuild_reasons(spec, plan, outcome),
        execution_failures: summary::render_execution_failures(outcome),
    }
}
