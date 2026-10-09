mod cli;
mod commands;
mod output;
#[cfg(feature = "tui")]
pub mod tui;

use gaia_artifact_providers::ArtifactProviderCatalog;
use gaia_default_providers::ProviderCatalogs;
use gaia_image_providers::ImageProviderCatalog;
use gaia_source_providers::SourceProviderCatalog;
use std::any::Any;
use std::panic::{self, AssertUnwindSafe};

pub use cli::{AppArgs, AppCommand, CacheArgs, CleanArgs, LockArgs};
pub use commands::{CommandOutcome, CommandResult, LockChange, LockReport, LockReportEntry};
use output::print_outcome;
pub use output::{backend_overview_lines, runtime_overview_lines};

#[derive(Default)]
pub struct AppContext {
    pub source_catalog: SourceProviderCatalog,
    pub artifact_catalog: ArtifactProviderCatalog,
    pub image_catalog: ImageProviderCatalog,
}

impl AppContext {
    pub fn with_defaults() -> Self {
        let (source_catalog, artifact_catalog, image_catalog) =
            ProviderCatalogs::with_defaults().into_parts();

        Self {
            source_catalog,
            artifact_catalog,
            image_catalog,
        }
    }
}

pub fn run() -> i32 {
    let args = AppArgs::from_env();
    let outcome = run_with_args(args);
    print_outcome(&outcome);
    outcome.exit_code()
}

pub fn run_with_args(args: AppArgs) -> CommandOutcome {
    let context = AppContext::with_defaults();
    match panic::catch_unwind(AssertUnwindSafe(|| commands::dispatch(&context, args))) {
        Ok(outcome) => outcome,
        Err(payload) => CommandOutcome::Failed {
            message: format!("command failed: {}", panic_message(payload.as_ref())),
        },
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    "unknown panic".into()
}

impl CommandOutcome {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Help { .. } | Self::Version { .. } => 0,
            Self::TuiExited { exit_code, .. } => *exit_code,
            Self::Failed { .. } => 1,
            Self::Validated { validation, .. } if !validation.errors.is_empty() => 2,
            Self::Planned { diagnostics, .. } if !diagnostics.is_empty() => 3,
            Self::Ran {
                report,
                validation,
                plan_diagnostics,
                ..
            } if report.summary.error_count > 0
                || !validation.errors.is_empty()
                || !plan_diagnostics.is_empty() =>
            {
                4
            }
            _ => 0,
        }
    }
}
