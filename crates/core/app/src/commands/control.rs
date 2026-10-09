//! `gaia pause|resume|cancel [run]`: signal a running `gaia run`. The run is
//! found in the registry (see `run_registry`) by its number or name from
//! `gaia status`, or by its build config. With no argument, the one live run
//! is used; when several are live, they are listed and the command fails
//! asking for a number or name. A build config that no registered run uses
//! is resolved, and its pid file in the build dir is used, as before.
//!
//! Pausing is Ctrl-Z, resuming `fg`, cancelling Ctrl-C (see `interrupt`). The
//! TUI monitor sends the same signals through [`signal_run`].

use std::io;
use std::path::Path;

use gaia_config::{ResolveOptions, try_resolve_config_with_options};

use crate::AppCommand;

use super::CommandOutcome;
use super::interrupt::{RUN_PID_FILE, running_gaia};
use super::live_status::unix_now;
use super::run_registry::{ListedRun, list_runs, select_run};
use super::status::numbered_lines;

/// What a control command acts on.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ControlTarget<'a> {
    /// A registered run that is live.
    Run(&'a ListedRun),
    /// No registered run: the build config's build dir is used.
    Build,
}

/// Chooses the target of `command` from the registered `runs`, for the run
/// named by `build` (`None` when no run was named).
pub(crate) fn control_target<'a>(
    runs: &'a [ListedRun],
    build: Option<&str>,
) -> Result<ControlTarget<'a>, String> {
    let Some(selector) = build else {
        let live: Vec<&ListedRun> = runs.iter().filter(|run| run.is_live()).collect();
        return match live.as_slice() {
            [] => Err(
                "no gaia run is live; pause, resume and cancel act on a run listed by 'gaia status'"
                    .into(),
            ),
            [only] => Ok(ControlTarget::Run(only)),
            many => Err(format!(
                "{} gaia runs are live; give the name or number of one (see 'gaia status'):\n{}",
                many.len(),
                numbered_lines(runs, unix_now(), true)
            )),
        };
    };
    match select_run(runs, selector)? {
        Some(run) if run.is_live() => Ok(ControlTarget::Run(run)),
        Some(run) => Err(format!(
            "build '{}' is not running (its run {} has ended)",
            run.run.display_name, run.run.pid
        )),
        None => Ok(ControlTarget::Build),
    }
}

pub fn control_command(
    build: Option<&str>,
    options: &ResolveOptions,
    command: AppCommand,
) -> CommandOutcome {
    let runs = list_runs();
    match control_target(&runs, build) {
        Ok(ControlTarget::Run(run)) => signal_listed(run, command),
        Ok(ControlTarget::Build) => control_build(build.unwrap_or_default(), options, command),
        Err(message) => CommandOutcome::Failed { message },
    }
}

fn signal_listed(run: &ListedRun, command: AppCommand) -> CommandOutcome {
    let pid = run.run.pid;
    if let Err(message) = signal_run(pid, command) {
        return CommandOutcome::Failed { message };
    }
    CommandOutcome::Text {
        text: format!(
            "{} build '{}' (gaia run {pid})",
            done_word(command),
            run.run.display_name
        ),
    }
}

/// A build config's run, found through its pid file in the build dir.
fn control_build(build: &str, options: &ResolveOptions, command: AppCommand) -> CommandOutcome {
    let spec = match try_resolve_config_with_options(build, options) {
        Ok(spec) => spec,
        Err(error) => {
            return CommandOutcome::Failed {
                message: error.to_string(),
            };
        }
    };
    let pid_file = Path::new(&spec.workspace.build_dir).join(RUN_PID_FILE);
    let Some(pid) = running_gaia(&pid_file) else {
        return CommandOutcome::Failed {
            message: format!(
                "build '{}' is not running (no live gaia run in '{}')",
                spec.identity.display_name,
                pid_file.display()
            ),
        };
    };
    if let Err(message) = signal_run(pid as u32, command) {
        return CommandOutcome::Failed { message };
    }
    CommandOutcome::Text {
        text: format!(
            "{} build '{}' (gaia run {pid})",
            done_word(command),
            spec.identity.display_name
        ),
    }
}

fn done_word(command: AppCommand) -> &'static str {
    match command {
        AppCommand::Pause => "paused",
        AppCommand::Resume => "resumed",
        _ => "cancelling",
    }
}

/// Sends the signals `command` (pause, resume or cancel) means to the live
/// `gaia run` `pid`. Cancelling continues a paused run first so it can stop
/// its commands.
pub(crate) fn signal_run(pid: u32, command: AppCommand) -> Result<(), String> {
    let pid = libc::pid_t::try_from(pid).map_err(|_| format!("invalid gaia run pid {pid}"))?;
    let signals: &[libc::c_int] = match command {
        AppCommand::Pause => &[libc::SIGTSTP],
        AppCommand::Resume => &[libc::SIGCONT],
        _ => &[libc::SIGCONT, libc::SIGINT],
    };
    for signal in signals {
        // SAFETY: signalling a pid read from the run's pid file, checked to
        // be a live gaia process.
        if unsafe { libc::kill(pid, *signal) } != 0 {
            return Err(format!(
                "failed to signal gaia run {pid}: {}",
                io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::live_status::{LiveRun, fixture_status};
    use crate::commands::run_registry::fixture_registered_run;

    fn live(name: &str, pid: u32) -> ListedRun {
        ListedRun {
            run: fixture_registered_run(pid, name),
            live: Some(LiveRun {
                pid,
                status: Some(fixture_status()),
                stopped: false,
            }),
            ended: None,
        }
    }

    fn ended(name: &str, pid: u32) -> ListedRun {
        ListedRun {
            run: fixture_registered_run(pid, name),
            live: None,
            ended: None,
        }
    }

    #[test]
    fn without_a_name_the_only_live_run_is_the_target() {
        let runs = vec![ended("Old", 1), live("New", 4242)];
        let target = control_target(&runs, None).expect("one live run");
        assert_eq!(target, ControlTarget::Run(&runs[1]));
    }

    #[test]
    fn without_a_name_several_live_runs_are_listed_and_none_is_chosen() {
        let runs = vec![live("One", 4242), ended("Old", 1), live("Two", 4343)];
        let message = control_target(&runs, None).expect_err("ambiguous");
        assert!(message.starts_with("2 gaia runs are live"), "{message}");
        assert!(message.contains(" 1  One  pid 4242"), "{message}");
        assert!(message.contains(" 3  Two  pid 4343"), "{message}");
        assert!(!message.contains("Old"), "{message}");
    }

    #[test]
    fn without_a_name_and_no_live_run_nothing_is_signalled() {
        let runs = vec![ended("Old", 1)];
        assert!(control_target(&runs, None).is_err());
        assert!(control_target(&[], None).is_err());
    }

    #[test]
    fn a_number_or_name_selects_a_live_run() {
        let runs = vec![live("One", 4242), live("Two", 4343)];
        assert_eq!(
            control_target(&runs, Some("2")).expect("number"),
            ControlTarget::Run(&runs[1])
        );
        assert_eq!(
            control_target(&runs, Some("one")).expect("name"),
            ControlTarget::Run(&runs[0])
        );
        assert!(control_target(&runs, Some("9")).is_err());
    }

    #[test]
    fn a_selected_run_that_ended_is_not_signalled() {
        let runs = vec![ended("Old", 1)];
        let message = control_target(&runs, Some("Old")).expect_err("not running");
        assert!(message.contains("is not running"), "{message}");
    }

    #[test]
    fn an_unregistered_build_config_falls_back_to_its_build_dir() {
        let runs = vec![live("One", 4242)];
        assert_eq!(
            control_target(&runs, Some("configs/builds/other.toml")).expect("fallback"),
            ControlTarget::Build
        );
    }
}
