//! Assembly steps run in dependency order (a step reading another's output
//! runs after it). Validation reports dependency cycles; a step declared
//! before what it reads is fine, since it runs after it.

use std::path::PathBuf;

use gaia_spec::{ImageAssemblySpec, assembly_step_paths, order_assembly_steps};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;

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
    }
}
