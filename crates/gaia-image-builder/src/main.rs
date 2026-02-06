use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;

use gaia_image_builder::Result;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Args {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Load config and print the computed task plan
    Plan {
        /// Path to a build definition TOML
        build: PathBuf,
        /// Print GraphViz dot instead of a linear plan
        #[arg(long)]
        dot: bool,
    },
    /// Load config, compute the plan, and execute it (currently a scaffold)
    Run {
        /// Path to a build definition TOML
        build: PathBuf,
        /// Print what would run without executing task bodies
        #[arg(long)]
        dry_run: bool,
        /// Max number of tasks to execute concurrently (0 = use CPU count)
        #[arg(long, default_value_t = 0)]
        max_parallel: usize,
    },
    /// Load config and print the fully-resolved TOML (after imports/extends)
    Resolve {
        /// Path to a build definition TOML
        build: PathBuf,
    },
    /// Terminal UI (build picker + config explorer + runner)
    Tui {
        /// Directory to scan for build definition TOMLs
        #[arg(long, default_value = "configs/builds")]
        builds_dir: PathBuf,
        /// Max number of tasks to execute concurrently (0 = use CPU count)
        #[arg(long, default_value_t = 0)]
        max_parallel: usize,
    },
}

fn main() -> Result<()> {
    let args = Args::parse();
    match args.cmd {
        Command::Plan { build, dot } => cmd_plan(&build, dot),
        Command::Run {
            build,
            dry_run,
            max_parallel,
        } => cmd_run(&build, dry_run, max_parallel),
        Command::Resolve { build } => cmd_resolve(&build),
        Command::Tui {
            builds_dir,
            max_parallel,
        } => gaia_image_builder::ui::run_tui(&builds_dir, max_parallel),
    }
}

fn cmd_plan(path: &PathBuf, dot: bool) -> Result<()> {
    let doc = gaia_image_builder::config::load(path.as_path())?;

    let mut plan = gaia_image_builder::planner::Plan::default();
    let modules = gaia_image_builder::modules::builtin_modules();
    for m in &modules {
        if m.detect(&doc) {
            m.plan(&doc, &mut plan)?;
        }
    }
    plan.finalize_default()?;

    if dot {
        print!("{}", plan.to_dot()?);
        return Ok(());
    }

    let ordered = plan.ordered()?;
    for (i, task) in ordered.iter().enumerate() {
        println!(
            "{:>2}. {:<22}  {:<10} {:<10}  {}",
            i + 1,
            task.id,
            task.module,
            task.phase,
            task.label
        );
    }
    Ok(())
}

fn cmd_resolve(path: &PathBuf) -> Result<()> {
    let doc = gaia_image_builder::config::load(path.as_path())?;
    // Best-effort pretty print of resolved config.
    let s = toml::to_string_pretty(&doc.value).unwrap_or_else(|_| format!("{:?}", doc.value));
    print!("{s}");
    Ok(())
}

fn cmd_run(path: &PathBuf, dry_run: bool, max_parallel: usize) -> Result<()> {
    let doc = gaia_image_builder::config::load(path.as_path())?;

    let mut plan = gaia_image_builder::planner::Plan::default();
    let modules = gaia_image_builder::modules::builtin_modules();
    for m in &modules {
        if m.detect(&doc) {
            m.plan(&doc, &mut plan)?;
        }
    }
    plan.finalize_default()?;

    let reg = gaia_image_builder::executor::builtin_registry()?;
    let sink = Arc::new(gaia_image_builder::executor::StdoutSink::default());
    let mut ctx = gaia_image_builder::executor::ExecCtx::new(dry_run, sink);

    let max_parallel = if max_parallel == 0 {
        num_cpus::get().max(1)
    } else {
        max_parallel.max(1)
    };

    if max_parallel <= 1 || dry_run {
        gaia_image_builder::executor::execute_plan(&doc, &plan, &reg, &mut ctx)?;
    } else {
        gaia_image_builder::executor::execute_plan_parallel(&doc, &plan, &reg, &ctx, max_parallel)?;
    }
    Ok(())
}
