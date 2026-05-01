mod clean;
mod plan;
mod resolve;
mod run;
mod state;
mod validate;

use gaia_exec::ExecutionError;
use gaia_exec::ExecutionOutcome;
use gaia_plan::{ExecutionPlan, PlanDiagnostic};
use gaia_report::{ReportBundle, ReportOutputBundle};
use gaia_spec::ResolvedBuildSpec;
use gaia_validate::ValidationReport;
use std::time::Duration;

use crate::{AppArgs, AppCommand, AppContext};
use gaia_config::ResolveOptions;

pub use clean::{CleanReport, clean_build_command};
pub use plan::plan_build_command;
pub use resolve::resolve_build_command;
pub use run::run_build_command;
pub use state::{load_reuse_state, save_reuse_state};
pub use validate::validate_build_command;

// Keep command outcomes value-typed so tests and callers can match complete
// results without chasing boxed variants through the command boundary.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    Help {
        text: String,
    },
    Version {
        text: String,
    },
    TuiExited {
        summary: String,
        exit_code: i32,
    },
    Resolved {
        spec: ResolvedBuildSpec,
    },
    Validated {
        spec: ResolvedBuildSpec,
        validation: ValidationReport,
    },
    Planned {
        spec: ResolvedBuildSpec,
        plan: ExecutionPlan,
        diagnostics: Vec<PlanDiagnostic>,
    },
    Cleaned {
        spec: ResolvedBuildSpec,
        report: CleanReport,
    },
    Ran {
        report: ReportBundle,
        report_outputs: ReportOutputBundle,
        post_build_output: Option<String>,
        run_duration: Duration,
        validation: ValidationReport,
        plan_diagnostics: Vec<PlanDiagnostic>,
        execution_errors: Vec<ExecutionError>,
    },
    Failed {
        message: String,
    },
}

pub type CommandResult = CommandOutcome;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunArtifacts {
    pub spec: ResolvedBuildSpec,
    pub validation: ValidationReport,
    pub plan: ExecutionPlan,
    pub plan_diagnostics: Vec<PlanDiagnostic>,
    pub outcome: ExecutionOutcome,
    pub report: ReportBundle,
    pub report_outputs: ReportOutputBundle,
    pub post_build_output: Option<String>,
    pub run_duration: Duration,
}

pub fn dispatch(context: &AppContext, args: AppArgs) -> CommandOutcome {
    match args.command {
        AppCommand::Help => CommandOutcome::Help { text: help_text() },
        AppCommand::Version => CommandOutcome::Version {
            text: version_text(),
        },
        AppCommand::Tui => run_tui_command(context, &args.build, &resolve_options(&args)),
        AppCommand::Resolve => resolve_build_command(&args.build, &resolve_options(&args)),
        AppCommand::Validate => {
            validate_build_command(context, &args.build, &resolve_options(&args))
        }
        AppCommand::Plan => plan_build_command(context, &args.build, &resolve_options(&args)),
        AppCommand::Clean => clean_build_command(&args.build, &resolve_options(&args), &args.clean),
        AppCommand::Run => run_build_command(context, &args.build, &resolve_options(&args)),
    }
}

#[cfg(feature = "tui")]
fn run_tui_command(context: &AppContext, build: &str, options: &ResolveOptions) -> CommandOutcome {
    crate::tui::run_tui_command(context, build, options)
}

#[cfg(not(feature = "tui"))]
fn run_tui_command(
    _context: &AppContext,
    _build: &str,
    _options: &ResolveOptions,
) -> CommandOutcome {
    CommandOutcome::Failed {
        message: "tui support is not enabled in this build".into(),
    }
}

fn help_text() -> String {
    [
        "gaia",
        "",
        "Usage:",
        "  gaia [run] [build-config]",
        "  gaia resolve [build-config]",
        "  gaia tui [build-config]",
        "  gaia validate [build-config]",
        "  gaia plan [build-config]",
        "  gaia clean [build-config]",
        "  gaia clean [build-config] --target build|out|all|configured",
        "  gaia clean [build-config] --profile <name>",
        "  gaia clean [build-config] --path <path>",
        "  gaia clean [build-config] --dry-run",
        "  gaia run [build-config]",
        "  gaia run [build-config] --preset <name>",
        "  gaia run [build-config] --env-file <path>",
        "  gaia run [build-config] --env KEY=VALUE",
        "  gaia run [build-config] --set key=value",
        "  gaia --help",
        "  gaia --version",
        "",
        "Default build config: examples/default-workspace/configs/default.toml",
    ]
    .join("\n")
}

fn version_text() -> String {
    format!("gaia {}", env!("CARGO_PKG_VERSION"))
}

fn resolve_options(args: &AppArgs) -> ResolveOptions {
    ResolveOptions {
        preset: args.preset.clone(),
        env_files: args.env_files.clone(),
        env_overrides: args.env_overrides.clone(),
        explicit_overrides: args.explicit_overrides.clone(),
    }
}
