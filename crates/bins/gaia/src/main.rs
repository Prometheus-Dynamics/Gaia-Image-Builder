//! The `gaia` command.
//!
//! - `gaia plan <product.toml>`: the graph and the plan, run nothing.
//! - `gaia build <product.toml>`: run the graph; each image output is
//!   printed with where it sits in the store, and the run is recorded.
//! - `gaia lock <product.toml>`: resolve the sources and write the lock
//!   (`gaia_lock_cmd`).
//! - `gaia clean`: remove from the store what the latest runs don't use.
//! - `gaia show <image>.<output>`: what an output of the latest run holds,
//!   and the steps it came from.
//!
//! Ctrl-C asks Daedalus to stop at the next step boundary; a second one
//! exits at once. Exit status: 0 done, 1 a step failed, 2 the command or a
//! description is wrong, 130 cancelled.

mod args;
mod build;
mod clean;
mod project;
mod record;
mod show;

use std::process::ExitCode;

/// A step failed.
const FAILED: u8 = 1;
/// The command line or a description is wrong.
const USAGE: u8 = 2;
/// Ctrl-C stopped the run.
const CANCELLED: u8 = 130;

/// Why a command stopped: the message and the exit status.
#[derive(Debug)]
struct Stop {
    code: u8,
    message: String,
}

impl Stop {
    fn usage(message: impl std::fmt::Display) -> Self {
        Self {
            code: USAGE,
            message: message.to_string(),
        }
    }
}

fn main() -> ExitCode {
    let words: Vec<String> = std::env::args().skip(1).collect();
    let result = args::parse(&words).and_then(|command| match command {
        args::Command::Help => {
            print!("{}", args::HELP);
            Ok(())
        }
        args::Command::Version => {
            println!("gaia {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        args::Command::Validate(options) => {
            for file in &options.files {
                let kind = gaia_describe::validate(file, options.kind).map_err(Stop::usage)?;
                println!("ok  {}  {}", kind.as_str(), file.display());
            }
            Ok(())
        }
        args::Command::Plan(options) => project::Project::open(&options)?.print_plan(options.json),
        args::Command::Build(options) => build::build(&options),
        args::Command::Clean(options) => clean::clean(&options),
        args::Command::Show(options) => show::show(&options),
        args::Command::Lock(words) => {
            gaia_lock_cmd::command::run(&words).map_err(|(code, message)| Stop { code, message })
        }
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(stop) => {
            if !stop.message.is_empty() {
                eprintln!("gaia: {}", stop.message);
            }
            ExitCode::from(stop.code)
        }
    }
}
