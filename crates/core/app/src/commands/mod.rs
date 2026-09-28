mod clean;
mod plan;
mod resolve;
mod run;
mod state;
mod validate;

use gaia_exec::ExecutionError;
use gaia_exec::ExecutionOutcome;
use gaia_plan::{ExecutionPlan, PlanDiagnostic, PlanTarget};
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
    let mut usage_errors = args.usage_errors.clone();
    let mut targets = Vec::new();
    for target in &args.only {
        match target.parse::<PlanTarget>() {
            Ok(target) => targets.push(target),
            Err(error) => usage_errors.push(error),
        }
    }
    if !args.only.is_empty() && !matches!(args.command, AppCommand::Run | AppCommand::Plan) {
        usage_errors.push("--only applies to 'run' and 'plan'".into());
    }
    if !usage_errors.is_empty() {
        return CommandOutcome::Failed {
            message: format!(
                "invalid arguments: {}\nrun 'gaia --help' for usage",
                usage_errors.join("; ")
            ),
        };
    }
    match args.command {
        AppCommand::Help => CommandOutcome::Help { text: help_text() },
        AppCommand::Version => CommandOutcome::Version {
            text: version_text(),
        },
        AppCommand::Tui => run_tui_command(context, &args),
        AppCommand::Resolve => resolve_build_command(&args.build, &resolve_options(&args)),
        AppCommand::Validate => {
            validate_build_command(context, &args.build, &resolve_options(&args))
        }
        AppCommand::Plan => {
            plan_build_command(context, &args.build, &resolve_options(&args), &targets)
        }
        AppCommand::Clean => clean_build_command(&args.build, &resolve_options(&args), &args.clean),
        AppCommand::Run => {
            run_build_command(context, &args.build, &resolve_options(&args), &targets)
        }
    }
}

#[cfg(feature = "tui")]
fn run_tui_command(context: &AppContext, args: &AppArgs) -> CommandOutcome {
    crate::tui::run_tui_command(
        context,
        crate::tui::TuiLaunch {
            build: &args.build,
            build_explicit: args.build_explicit,
            builds_dir: args.builds_dir.as_deref(),
        },
        &resolve_options(args),
    )
}

#[cfg(not(feature = "tui"))]
fn run_tui_command(_context: &AppContext, _args: &AppArgs) -> CommandOutcome {
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
        "  gaia tui --builds-dir <dir>",
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
        "  gaia run [build-config] --only artifacts[,image,...]",
        "  gaia plan [build-config] --only artifact:<id>",
        "  gaia --help",
        "  gaia --version",
        "",
        "--only runs part of the build graph plus its dependencies. Targets are",
        "domains (sources, artifacts, install, stage, image, checkpoints) or",
        "operation ids from 'gaia plan'. Reuse state for the rest is kept.",
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
