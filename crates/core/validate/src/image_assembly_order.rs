//! Assembly steps run in dependency order (a step reading another's output
//! runs after it). Validation reports dependency cycles, and steps declared
//! (or of a kind classically run) before the step producing what they
//! read: older Gaia versions ran those first, on the previous run's file.

use std::path::PathBuf;

use gaia_spec::{
    ImageAssemblySpec, assembly_step_paths, assembly_steps_reading_later_outputs,
    order_assembly_steps,
};

use crate::ValidationDiagnostic;
use crate::diagnostics::{error, warning};

pub(crate) fn validate_assembly_order(
    assembly: &ImageAssemblySpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    // Paths as written: enough to see which steps share them.
    let paths = assembly_step_paths(
        assembly,
        &|template| Some(PathBuf::from(template.as_str())),
        &|tree| {
            assembly
                .trees
                .iter()
                .find(|candidate| candidate.id.as_str() == tree)
                .map(|candidate| PathBuf::from(candidate.path.as_str()))
        },
    );
    if let Err(cycle) = order_assembly_steps(&paths) {
        diagnostics.push(error(
            "assembly_step_cycle",
            cycle.describe(assembly),
            Some("image.assembly".into()),
        ));
        return;
    }
    for (reader, writer) in assembly_steps_reading_later_outputs(&paths) {
        diagnostics.push(warning(
            "assembly_reads_later_output",
            format!(
                "assembly {} reads what {} produces, so it now runs after it; Gaia \
                 versions without dependency-ordered assembly ran it first, on the \
                 previous run's file",
                reader.describe(assembly),
                writer.describe(assembly)
            ),
            Some("image.assembly".into()),
        ));
    }
}
