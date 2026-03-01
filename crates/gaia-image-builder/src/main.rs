use clap::{ArgAction, Args as ClapArgs, Parser, Subcommand};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gaia_image_builder::{Error, Result};

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct CliArgs {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Debug, Clone, Default, ClapArgs)]
struct InputOverrides {
    /// Override a build input (repeatable): --set key=value
    #[arg(long = "set", action = ArgAction::Append, value_name = "KEY=VALUE")]
    set: Vec<String>,
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
        #[command(flatten)]
        inputs: InputOverrides,
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
        #[command(flatten)]
        inputs: InputOverrides,
    },
    /// Load config and print the fully-resolved TOML (after imports/extends)
    Resolve {
        /// Path to a build definition TOML
        build: PathBuf,
        #[command(flatten)]
        inputs: InputOverrides,
    },
    /// Initialize a minimal Gaia image scaffold
    Init {
        /// Target directory (default: ./gaia)
        dir: Option<PathBuf>,
        /// Overwrite existing scaffold files if they already exist
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Checkpoint operations
    Checkpoints {
        #[command(subcommand)]
        cmd: CheckpointsCommand,
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

#[derive(Debug, Subcommand)]
enum CheckpointsCommand {
    /// Show checkpoint decisions for a build
    Status {
        /// Path to a build definition TOML
        build: PathBuf,
        #[command(flatten)]
        inputs: InputOverrides,
    },
    /// Retry failed/pending checkpoint uploads
    Retry {
        /// Path to a build definition TOML
        build: PathBuf,
        /// Maximum number of queue items to retry (default: all)
        #[arg(long)]
        max: Option<usize>,
        #[command(flatten)]
        inputs: InputOverrides,
    },
    /// List checkpoint objects and fingerprints (local and optional remote backend state)
    List {
        /// Path to a build definition TOML
        build: PathBuf,
        /// Include remote backend listing
        #[arg(long, default_value_t = false)]
        remote: bool,
        /// Filter to a single checkpoint id
        #[arg(long)]
        id: Option<String>,
        #[command(flatten)]
        inputs: InputOverrides,
    },
}

fn main() -> Result<()> {
    let _ = dotenv::dotenv();
    let args = CliArgs::parse();
    match args.cmd {
        Command::Plan { build, dot, inputs } => cmd_plan(&build, dot, &inputs),
        Command::Run {
            build,
            dry_run,
            max_parallel,
            inputs,
        } => cmd_run(&build, dry_run, max_parallel, &inputs),
        Command::Resolve { build, inputs } => cmd_resolve(&build, &inputs),
        Command::Init { dir, force } => cmd_init(dir, force),
        Command::Checkpoints { cmd } => cmd_checkpoints(cmd),
        Command::Tui {
            builds_dir,
            max_parallel,
        } => gaia_image_builder::ui::run_tui(&builds_dir, max_parallel),
    }
}

fn build_plan(
    doc: &gaia_image_builder::config::ConfigDoc,
) -> Result<gaia_image_builder::planner::Plan> {
    let mut plan = gaia_image_builder::planner::Plan::default();
    let modules = gaia_image_builder::modules::builtin_modules();
    for m in &modules {
        if m.detect(doc) {
            m.plan(doc, &mut plan)?;
        }
    }
    gaia_image_builder::checkpoints::validate_against_plan(doc, &plan)?;
    plan.finalize_default()?;
    Ok(plan)
}

fn load_doc(
    path: &PathBuf,
    inputs: &InputOverrides,
) -> Result<gaia_image_builder::config::ConfigDoc> {
    let mut doc = gaia_image_builder::config::load(path.as_path())?;
    gaia_image_builder::build_inputs::apply_cli_overrides(&mut doc, &inputs.set)?;
    Ok(doc)
}

fn cmd_plan(path: &PathBuf, dot: bool, inputs: &InputOverrides) -> Result<()> {
    let doc = load_doc(path, inputs)?;
    let plan = build_plan(&doc)?;

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

fn cmd_resolve(path: &PathBuf, inputs: &InputOverrides) -> Result<()> {
    let doc = load_doc(path, inputs)?;
    // Best-effort pretty print of resolved config.
    let s = toml::to_string_pretty(&doc.value).unwrap_or_else(|_| format!("{:?}", doc.value));
    print!("{s}");
    Ok(())
}

fn cmd_init(dir: Option<PathBuf>, force: bool) -> Result<()> {
    let target = dir.unwrap_or_else(|| PathBuf::from("gaia"));
    if target.exists() && !target.is_dir() {
        return Err(Error::msg(format!(
            "init target exists but is not a directory: {}",
            target.display()
        )));
    }
    fs::create_dir_all(&target)
        .map_err(|e| Error::msg(format!("failed to create {}: {e}", target.display())))?;

    let workspace_root = init_workspace_root_value(&target)?;
    let files = init_scaffold_files(&workspace_root);

    let collisions = files
        .iter()
        .map(|(rel, _)| target.join(rel))
        .filter(|p| p.exists())
        .collect::<Vec<_>>();
    if !force && !collisions.is_empty() {
        let list = collisions
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Error::msg(format!(
            "refusing to overwrite existing files: {list}. rerun with --force to overwrite scaffold files"
        )));
    }

    for (rel, content) in files {
        let path = target.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| Error::msg(format!("failed to create {}: {e}", parent.display())))?;
        }
        fs::write(&path, content)
            .map_err(|e| Error::msg(format!("failed to write {}: {e}", path.display())))?;
    }

    let build_file = target.join("build.toml");
    println!("initialized Gaia scaffold at {}", target.display());
    println!("build file: {}", build_file.display());
    println!("next:");
    println!("  gaia resolve {}", build_file.display());
    println!("  gaia plan {}", build_file.display());
    println!("  gaia run {} --dry-run", build_file.display());
    Ok(())
}

fn init_workspace_root_value(target: &Path) -> Result<String> {
    if target == Path::new(".") {
        return Ok(".".to_string());
    }
    if target.is_absolute() {
        return Ok(target.display().to_string());
    }
    target
        .to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| Error::msg("init target path is not valid UTF-8"))
}

fn init_scaffold_files(workspace_root: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "build.toml",
            r#"imports = [
  "./configs/workspace.toml",
  "./configs/buildroot.toml",
  "./configs/stage.toml",
]
"#
            .to_string(),
        ),
        (
            "configs/workspace.toml",
            format!(
                r#"[workspace]
root_dir = "{workspace_root}"
"#
            ),
        ),
        (
            "configs/buildroot.toml",
            r#"[buildroot]
version = "2025.11"
defconfig = "raspberrypicm5io_defconfig"
"#
            .to_string(),
        ),
        (
            "configs/stage.toml",
            r#"[stage]

[[stage.files]]
src = "assets/etc/hostname"
dst = "/etc/hostname"
mode = 420

[[stage.files]]
src = "assets/etc/os-release"
dst = "/etc/os-release"
mode = 420

[[stage.files]]
src = "assets/etc/motd"
dst = "/etc/motd"
mode = 420
"#
            .to_string(),
        ),
        ("assets/etc/hostname", "gaia-image\n".to_string()),
        (
            "assets/etc/os-release",
            r#"NAME="Gaia Image"
ID=gaia
PRETTY_NAME="Gaia Image"
"#
            .to_string(),
        ),
        ("assets/etc/motd", "Built with Gaia.\n".to_string()),
    ]
}

fn cmd_run(
    path: &PathBuf,
    dry_run: bool,
    max_parallel: usize,
    inputs: &InputOverrides,
) -> Result<()> {
    let doc = load_doc(path, inputs)?;
    let plan = build_plan(&doc)?;

    let checkpoint_status = gaia_image_builder::checkpoints::status_for_doc(&doc)?;
    if !checkpoint_status.is_empty() {
        print!(
            "{}",
            gaia_image_builder::checkpoints::format_status_report(&checkpoint_status)
        );
    }

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

fn cmd_checkpoints(cmd: CheckpointsCommand) -> Result<()> {
    match cmd {
        CheckpointsCommand::Status { build, inputs } => {
            let doc = load_doc(&build, &inputs)?;
            let plan = build_plan(&doc)?;
            // Validate against current plan before reporting.
            gaia_image_builder::checkpoints::validate_against_plan(&doc, &plan)?;
            let items = gaia_image_builder::checkpoints::status_for_doc(&doc)?;
            print!(
                "{}",
                gaia_image_builder::checkpoints::format_status_report(&items)
            );
            Ok(())
        }
        CheckpointsCommand::Retry { build, max, inputs } => {
            let doc = load_doc(&build, &inputs)?;
            let report = gaia_image_builder::checkpoints::retry_pending_uploads(&doc, max)?;
            println!(
                "checkpoint retry: attempted={} uploaded={} failed={}",
                report.attempted, report.uploaded, report.failed
            );
            Ok(())
        }
        CheckpointsCommand::List {
            build,
            remote,
            id,
            inputs,
        } => {
            let doc = load_doc(&build, &inputs)?;
            let items = gaia_image_builder::checkpoints::list_for_doc(&doc, remote, id.as_deref())?;
            print!(
                "{}",
                gaia_image_builder::checkpoints::format_list_report(&items, remote)
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_workspace_root_value_for_default_relative_dir() {
        let value = init_workspace_root_value(Path::new("gaia")).expect("root value");
        assert_eq!(value, "gaia");
    }

    #[test]
    fn init_uses_custom_target_and_workspace_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("my-image");

        cmd_init(Some(target.clone()), false).expect("init custom");
        let workspace =
            fs::read_to_string(target.join("configs/workspace.toml")).expect("read workspace");
        let expect_line = format!("root_dir = \"{}\"", target.display());
        assert!(
            workspace.contains(&expect_line),
            "workspace content: {workspace}"
        );
        assert!(target.join("build.toml").is_file());
        assert!(target.join("configs/buildroot.toml").is_file());
        assert!(target.join("configs/stage.toml").is_file());
        assert!(target.join("assets/etc/hostname").is_file());
    }

    #[test]
    fn init_refuses_overwrite_without_force() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let target = tmp.path().join("gaia");

        fs::create_dir_all(&target).expect("mkdir gaia");
        fs::write(target.join("build.toml"), "custom").expect("seed build.toml");

        let err = cmd_init(Some(target.clone()), false).expect_err("expected conflict");
        let msg = err.to_string();
        assert!(
            msg.contains("refusing to overwrite existing files"),
            "unexpected error: {msg}"
        );

        cmd_init(Some(target.clone()), true).expect("force overwrite");
        let rebuilt = fs::read_to_string(target.join("build.toml")).expect("read build.toml");
        assert!(
            rebuilt.contains("./configs/workspace.toml"),
            "unexpected build.toml: {rebuilt}"
        );
    }
}
