mod artifacts;
mod checkpoints;
mod core;
mod diagnostics;
mod image;
mod image_assembly;
mod inputs;
mod install_stage;
mod model;
mod providers;
mod reporting;
mod sources;
mod workspace;

pub use core::validate_spec;
pub use model::{DiagnosticSeverity, ValidationDiagnostic, ValidationReport};
pub use providers::validate_spec_with_providers;
