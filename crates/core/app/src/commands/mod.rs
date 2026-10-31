mod cache;
mod cache_remote;
mod cache_stats;
mod clean;
#[cfg(unix)]
pub(crate) mod control;
mod disk_usage;
mod interrupt;
mod layer_delta;
pub(crate) mod live_status;
mod lock;
mod plan;
mod preview;
mod progress;
mod progress_ci;
mod rebuild;
mod resolve;
mod reuse_reasons;
mod run;
pub(crate) mod run_registry;
mod state;
pub(crate) mod status;
#[cfg(unix)]
mod status_json;
mod validate;
mod variants;

use gaia_exec::ExecutionError;
use gaia_exec::ExecutionOutcome;
use gaia_plan::{ExecutionPlan, PlanDiagnostic, PlanTarget, RebuildRequest};
use gaia_report::{ReportBundle, ReportOutputBundle};
use gaia_spec::ResolvedBuildSpec;
use gaia_validate::ValidationReport;
use std::time::Duration;

use crate::{AppArgs, AppCommand, AppContext};
use gaia_config::ResolveOptions;

pub(crate) use cache::cache_command;
pub(crate) use clean::{CleanReport, clean_build_command};
pub(crate) use lock::{LockChange, LockReport, lock_build_command};
pub(crate) use plan::plan_build_command;
pub(crate) use preview::{PreviewReport, preview_build_command};
pub(crate) use preview::{preview_json, print_preview};
pub(crate) use resolve::resolve_build_command;
pub(crate) use run::run_build_command;
pub(crate) use state::{load_operation_durations, load_reuse_state, save_reuse_state};
pub(crate) use validate::validate_build_command;

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
        /// Duration estimate from the last recorded operation timings.
        estimate: gaia_plan::PlanEstimate,
    },
    /// `gaia preview`: what a run would do.
    Previewed {
        report: PreviewReport,
    },
    Cleaned {
        spec: ResolvedBuildSpec,
        report: CleanReport,
    },
    /// A command's plain-text report (`gaia cache`, `gaia pause`, ...).
    Text {
        text: String,
    },
    /// A command's report as the command-specific fields of its `--json`
    /// document (`gaia status --json`, `gaia cache --json`).
    Json {
        value: serde_json::Value,
    },
    Locked {
        spec: ResolvedBuildSpec,
        report: LockReport,
    },
    /// Several variants of one build (`[[variants]]`), each with its own
    /// outcome. `header` lines come first; the outcomes follow in run order.
    Variants {
        header: Vec<String>,
        outcomes: Vec<VariantOutcome>,
    },
    Ran {
        report: ReportBundle,
        report_outputs: ReportOutputBundle,
        post_build_output: Option<String>,
        run_duration: Duration,
        validation: ValidationReport,
        plan_diagnostics: Vec<PlanDiagnostic>,
        execution_errors: Vec<ExecutionError>,
        /// The export directory the config sets for this build, if any; a
        /// `--export` on the command line takes precedence over it.
        configured_export_dir: Option<String>,
    },
    Failed {
        message: String,
    },
}

/// The outcome of one variant of a build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantOutcome {
    pub id: String,
    pub outcome: CommandOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunArtifacts {
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
    let mut rebuild = RebuildRequest {
        targets: Vec::new(),
        packages: args.rebuild_packages.clone(),
    };
    for target in &args.rebuild {
        match target.parse::<PlanTarget>() {
            Ok(target) => rebuild.targets.push(target),
            Err(error) => usage_errors.push(error),
        }
    }
    if !rebuild.is_empty()
        && !matches!(
            args.command,
            AppCommand::Run | AppCommand::Plan | AppCommand::Preview
        )
    {
        usage_errors
            .push("--rebuild and --rebuild-package apply to 'run', 'plan' and 'preview'".into());
    }
    if args.follow && args.command != AppCommand::Status {
        usage_errors.push("--follow applies to 'status'".into());
    }
    if !args.only.is_empty()
        && !matches!(
            args.command,
            AppCommand::Run | AppCommand::Plan | AppCommand::Preview
        )
    {
        usage_errors.push("--only applies to 'run', 'plan' and 'preview'".into());
    }
    if args.fail_on_clean && args.command != AppCommand::Preview {
        usage_errors.push("--fail-on-clean applies to 'preview' (or 'run --dry-run')".into());
    }
    if args.json
        && !matches!(
            args.command,
            AppCommand::Preview
                | AppCommand::Run
                | AppCommand::Plan
                | AppCommand::Status
                | AppCommand::Validate
                | AppCommand::Cache
                | AppCommand::LayerDelta
        )
    {
        usage_errors.push(
            "--json applies to 'run', 'preview', 'plan', 'status', 'validate', 'cache' and 'layer delta'".into(),
        );
    }
    if args.json && args.follow {
        usage_errors.push("--json cannot be combined with --follow".into());
    }
    if args.ci && args.command != AppCommand::Run {
        usage_errors.push("--ci applies to 'run'".into());
    }
    if args.export_dir.is_some() && args.command != AppCommand::Run {
        usage_errors.push("--export applies to 'run'".into());
    }
    if args.lock.update && args.command != AppCommand::Lock {
        usage_errors.push("--update applies to 'lock'".into());
    }
    if args.clean.all_caches && args.command != AppCommand::Clean {
        usage_errors.push("--all-caches applies to 'clean'".into());
    }
    if args.clean.unused_mirrors && args.command != AppCommand::Clean {
        usage_errors.push("--unused-mirrors applies to 'clean'".into());
    }
    let cache = &args.cache;
    if (cache.level.is_some()
        || cache.clear.is_some()
        || !cache.packages.is_empty()
        || !cache.remove.is_empty()
        || cache.remove_legacy
        || cache.stats)
        && args.command != AppCommand::Cache
    {
        usage_errors.push(
            "--level, --package, --remove, --remove-legacy and --clear apply to 'cache'".into(),
        );
    }
    if cache.clear.is_some() && (!cache.remove.is_empty() || cache.remove_legacy) {
        usage_errors.push("use either --remove or --clear".into());
    }
    let remote_flags = cache.remote || cache.checkpoint.is_some() || cache.keep_days.is_some();
    if (remote_flags || cache.prune) && args.command != AppCommand::Cache {
        usage_errors
            .push("--prune, --remote, --checkpoint and --keep-days apply to 'cache'".into());
    }
    if cache.prune && !cache.remote {
        usage_errors.push("'cache --prune' needs --remote (the registry); the local cache is removed with --remove or --clear".into());
    }
    if remote_flags && !cache.prune {
        usage_errors.push("--remote, --checkpoint and --keep-days apply to 'cache --prune'".into());
    }
    if cache.prune && (cache.clear.is_some() || !cache.remove.is_empty() || cache.stats) {
        usage_errors.push("'cache --prune' takes no --remove, --clear or --stats".into());
    }
    if cache.stats
        && (cache.clear.is_some()
            || !cache.remove.is_empty()
            || cache.remove_legacy
            || cache.level.is_some()
            || !cache.packages.is_empty())
    {
        usage_errors.push("'cache stats' takes no --level, --package, --remove or --clear".into());
    }
    if args.offline
        && !matches!(
            args.command,
            AppCommand::Run | AppCommand::Plan | AppCommand::Preview
        )
    {
        usage_errors.push("--offline applies to 'run', 'plan', 'preview' and 'prefetch'".into());
    }
    if args.prefetch {
        if args.command != AppCommand::Run {
            usage_errors.push("--prefetch applies to 'run' and 'prefetch' (not --dry-run)".into());
        }
        if args.offline {
            usage_errors.push("--prefetch needs the network; drop --offline".into());
        }
        if !args.only.is_empty() || !rebuild.is_empty() || args.export_dir.is_some() {
            usage_errors.push(
                "--prefetch downloads only; it takes no --only, --rebuild, --rebuild-package or --export".into(),
            );
        }
    }
    if !usage_errors.is_empty() {
        return CommandOutcome::Failed {
            message: format!(
                "invalid arguments: {}\nrun 'gaia --help' for usage",
                usage_errors.join("; ")
            ),
        };
    }
    let needs_build = !matches!(
        args.command,
        AppCommand::Help
            | AppCommand::Version
            | AppCommand::ConfigSchema
            | AppCommand::ConfigKeys
            | AppCommand::Tui
            | AppCommand::Pause
            | AppCommand::Resume
            | AppCommand::Cancel
            | AppCommand::Status
            | AppCommand::LayerDelta
    );
    if needs_build && args.build.is_empty() {
        return CommandOutcome::Failed {
            message: format!(
                "{}\nrun 'gaia --help' for usage",
                crate::cli::NO_BUILD_CONFIG_ERROR
            ),
        };
    }
    // The build named on the command line; `None` when the command runs
    // without one (status, pause, resume and cancel then use the registry).
    let explicit_build = args.build_explicit.then_some(args.build.as_str());
    if args.command == AppCommand::Run {
        progress_ci::set_ci_output(args.ci || ci_from_env());
    }
    match args.command {
        AppCommand::Help => CommandOutcome::Help { text: help_text() },
        AppCommand::Version => CommandOutcome::Version {
            text: version_text(),
        },
        AppCommand::Tui => run_tui_command(context, &args),
        AppCommand::ConfigSchema => CommandOutcome::Text {
            text: gaia_config::config_schema_json(),
        },
        AppCommand::ConfigKeys => CommandOutcome::Text {
            text: gaia_config::config_key_reference_markdown(),
        },
        AppCommand::Resolve => resolve_build_command(&args.build, &resolve_options(&args)),
        AppCommand::Validate => validate_build_command(
            context,
            &args.build,
            &resolve_options(&args),
            &args.variants,
        ),
        AppCommand::Plan => plan_build_command(
            context,
            &args.build,
            &resolve_options(&args),
            &targets,
            &rebuild,
            &args.variants,
        ),
        AppCommand::Preview => preview_build_command(
            context,
            &args.build,
            &resolve_options(&args),
            &targets,
            &rebuild,
            args.fail_on_clean,
            args.json,
            &args.variants,
        ),
        AppCommand::Clean => clean_build_command(&args.build, &resolve_options(&args), &args.clean),
        #[cfg(unix)]
        AppCommand::Pause | AppCommand::Resume | AppCommand::Cancel => {
            control::control_command(explicit_build, &resolve_options(&args), args.command)
        }
        #[cfg(not(unix))]
        AppCommand::Pause | AppCommand::Resume | AppCommand::Cancel => CommandOutcome::Failed {
            message: "pause, resume and cancel need a Unix system".into(),
        },
        AppCommand::Status => status::status_command(
            explicit_build,
            &resolve_options(&args),
            args.follow,
            args.json,
        ),
        AppCommand::Cache => cache_command(
            &args.build,
            args.build_explicit,
            &resolve_options(&args),
            &args.cache,
            args.json,
        ),
        AppCommand::LayerDelta => layer_delta::layer_delta_command(
            &args.build,
            &args.layer.from,
            args.layer.out.as_deref(),
            args.layer.max_window.as_deref(),
            args.json,
        ),
        AppCommand::Lock => lock_build_command(&args.build, &resolve_options(&args), &args.lock),
        AppCommand::Run => run_build_command(
            context,
            &args.build,
            &resolve_options(&args),
            &targets,
            &rebuild,
            &args.variants,
            args.prefetch,
        ),
    }
}

/// `CI` set to anything but empty, `0` or `false`, as CI services do.
fn ci_from_env() -> bool {
    std::env::var("CI").is_ok_and(|value| {
        !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false"
        )
    })
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
        "  gaia clean [build-config] --target caches [--unused-mirrors] [--all-caches]",
        "  gaia config schema                 JSON Schema of the build config, for editors (taplo, Even Better TOML)",
        "  gaia config keys                   Every table and key a build file takes (docs/reference/config-keys.md)",
        "  gaia cache [build-config] [--list] [--level system|project] [--package <glob>]",
        "  gaia cache [build-config] --remove <package>[@key-prefix][,...] [--dry-run]",
        "  gaia cache [build-config] --remove-legacy [--dry-run]",
        "  gaia cache [build-config] --clear system|project|ccache [--dry-run]",
        "  gaia cache [build-config] --prune --remote --checkpoint <id> [--keep-days 30] [--dry-run]",
        "  gaia cache stats [build-config]   (disk usage of every cache Gaia keeps)",
        "  gaia pause [run]               (or Ctrl-Z in the running gaia run)",
        "  gaia resume [run]              (or fg)",
        "  gaia cancel [run]              (or Ctrl-C; finished work is kept)",
        "  gaia status [run] [--follow]   (lists every gaia run; or one of them)",
        "  gaia status <build-config> [--follow]",
        "  gaia lock [build-config]",
        "  gaia lock [build-config] --update [source-id[,source-id...]]",
        "  gaia run [build-config]",
        "  gaia run [build-config] --preset <name>",
        "  gaia run [build-config] --env-file <path>",
        "  gaia run [build-config] --env KEY=VALUE",
        "  gaia run [build-config] --set key=value",
        "  gaia run [build-config] --variant <id>[,<id>]  (a build with [[variants]]; default = base)",
        "  gaia run [build-config] --only artifacts[,image,...]",
        "  gaia run [build-config] --rebuild <operation>[,...]  (run these even if reusable)",
        "  gaia run [build-config] --rebuild-package <package>[,...]  (Buildroot)",
        "  gaia run [build-config] --export <dir>  (copy the image after a successful run)",
        "  gaia plan [build-config] --only artifact:<id>",
        "  gaia preview [build-config] [--set key=value] [--only ...] [--json] [--fail-on-clean]",
        "  gaia run [build-config] --dry-run     (same as 'gaia preview')",
        "  gaia run [build-config] --ci     (GitHub Actions log groups; also when CI is set)",
        "  gaia prefetch [build-config]     (download sources and Buildroot packages; no build)",
        "  gaia run [build-config] --prefetch  (same as 'gaia prefetch')",
        "  gaia run [build-config] --offline   (no network: the caches must be complete)",
        "  gaia layer delta <layers-dir> --from <old-layers-dir>[,...] --out <dir> [--max-window <size>]",
        "                                  (zstd deltas of [image.layers] layers against earlier releases)",
        "  gaia --help",
        "  gaia --version",
        "",
        "--json prints one JSON document on stdout for 'run', 'preview', 'plan',",
        "'status', 'validate', 'cache' and 'layer delta' (schema_version 1, see docs/cli.md);",
        "progress and logs stay on stderr.",
        "",
        "'preview' shows what 'run' would do without changing anything: which",
        "operations run or are reused and why, what the Buildroot clean would",
        "delete or rebuild, which packages come from the cache, and every path a",
        "run would remove. --fail-on-clean exits 3 when a run would clean the",
        "whole tree or delete anything but leftovers of an earlier clean.",
        "",
        "--only runs part of the build graph plus its dependencies. Targets are",
        "domains (sources, artifacts, install, stage, image, checkpoints) or",
        "operation ids from 'gaia plan'. Reuse state for the rest is kept.",
        "",
        "--rebuild runs the named operations (ids, domains, or an id with '*' such",
        "as artifact:*) whatever the reuse state says, with the reason rebuild_requested.",
        "Their dependents follow as usual: content-based reuse may still skip what",
        "the rebuilt outputs leave unchanged. --rebuild-package dircleans the named",
        "Buildroot packages before make and builds them again instead of restoring",
        "them from the package cache, and so makes image:prepare and image:build run.",
        "'gaia resume' does not repeat --rebuild or --rebuild-package.",
        "",
        "'pause', 'resume', 'cancel' and 'status' find the gaia runs on this system",
        "from any directory: with no run named, 'status' lists them all, and the",
        "signal commands act on the one live run. A run is named by its number in",
        "the 'status' list, its build name or its build config path.",
        "",
        "'gaia lock' records the commit of every git source in <build>.gaia.lock",
        "next to the build file; builds then check out exactly those commits.",
        "'clean --target caches' prunes stale shared cargo target dirs and leftover",
        "work dirs; --unused-mirrors also prunes git mirrors this build does not use,",
        "and --all-caches removes every git mirror and the shared download and tool caches.",
        "",
        "Without [build-config], gaia uses build.toml in the current directory, else",
        "its first other *.toml file by name (not Cargo.toml or rust-toolchain.toml).",
        "With neither, commands that need a build fail; see examples/ for configs.",
    ]
    .join("\n")
}

fn version_text() -> String {
    format!("gaia {}", env!("CARGO_PKG_VERSION"))
}

fn resolve_options(args: &AppArgs) -> ResolveOptions {
    // `--offline` is `--set execution.offline=true`, so it wins over the
    // build config like any explicit override.
    let mut explicit_overrides = args.explicit_overrides.clone();
    if args.offline {
        explicit_overrides.push(("execution.offline".to_string(), "true".to_string()));
    }
    ResolveOptions {
        preset: args.preset.clone(),
        env_files: args.env_files.clone(),
        env_overrides: args.env_overrides.clone(),
        explicit_overrides,
        variant_presets: Vec::new(),
        selected_variants: args.variants.clone(),
        resolve_unpinned_import_sources: false,
    }
}
