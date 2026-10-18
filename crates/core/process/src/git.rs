//! The host git command every provider starts.

use std::process::Command;

/// A git command for the host. Git runs unattended: no prompts (so a missing
/// credential fails fast instead of hanging the build), and the repository
/// owner check is off because Gaia reads trees it does not own.
pub fn git_command() -> Command {
    let mut command = Command::new("git");
    command
        .arg("-c")
        .arg("safe.directory=*")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never");
    command
}
