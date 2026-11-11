//! The `gaia-lock` binary: `gaia lock` on its own. See [`gaia_lock_cmd::command`].

use std::process::ExitCode;

fn main() -> ExitCode {
    let words: Vec<String> = std::env::args().skip(1).collect();
    match gaia_lock_cmd::command::run(&words) {
        Ok(()) => ExitCode::SUCCESS,
        Err((code, message)) => {
            eprintln!("gaia lock: {message}");
            ExitCode::from(code)
        }
    }
}
