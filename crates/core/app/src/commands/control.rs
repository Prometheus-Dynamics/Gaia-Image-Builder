//! `gaia pause|resume|cancel [build]`: signal the running `gaia run` of a
//! build, found by the pid it publishes under the build dir. Pausing is
//! Ctrl-Z, resuming `fg`, cancelling Ctrl-C (see `interrupt`).

use std::path::Path;

use gaia_config::{ResolveOptions, try_resolve_config_with_options};

use crate::AppCommand;

use super::CommandOutcome;
use super::interrupt::RUN_PID_FILE;

pub fn control_command(
    build: &str,
    options: &ResolveOptions,
    command: AppCommand,
) -> CommandOutcome {
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
    let (signals, done): (&[libc::c_int], &str) = match command {
        AppCommand::Pause => (&[libc::SIGTSTP], "paused"),
        AppCommand::Resume => (&[libc::SIGCONT], "resumed"),
        // Continue a paused run first so it can stop its commands.
        _ => (&[libc::SIGCONT, libc::SIGINT], "cancelling"),
    };
    for signal in signals {
        // SAFETY: signalling a pid read from the run's pid file, checked to
        // be a live gaia process.
        if unsafe { libc::kill(pid, *signal) } != 0 {
            return CommandOutcome::Failed {
                message: format!(
                    "failed to signal gaia run {pid}: {}",
                    std::io::Error::last_os_error()
                ),
            };
        }
    }
    CommandOutcome::Text {
        text: format!(
            "{done} build '{}' (gaia run {pid})",
            spec.identity.display_name
        ),
    }
}

/// The pid in `pid_file`, when it is a live process named gaia.
fn running_gaia(pid_file: &Path) -> Option<libc::pid_t> {
    let pid = std::fs::read_to_string(pid_file)
        .ok()?
        .trim()
        .parse::<libc::pid_t>()
        .ok()
        .filter(|pid| *pid > 0)?;
    // SAFETY: signal 0 only checks that the process exists.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return None;
    }
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
    (comm.is_empty() || comm.trim().starts_with("gaia")).then_some(pid)
}
