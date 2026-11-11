//! Runs the A2 graph once, in this process, over a store that may already
//! hold some of its steps:
//!
//! ```text
//! gaia-second-run <store> <git> <toolchain> (<label> <url> <commit> <bin>)...
//! ```
//!
//! Each chain clones `url` at `commit`, builds `bin` in release and packs it
//! at `usr/bin/<bin>`. Prints `<label> <tar object>` for each chain. Which
//! steps ran and which were found is in the step record next to the store
//! (`gaia_step::step_records`). Exits 1 when the run faults.

use gaia_proof::{Chain, Setup, digest_of, graph, load, run, take_tar};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 7 || !(args.len() - 3).is_multiple_of(4) {
        eprintln!(
            "usage: gaia-second-run <store> <git> <toolchain> (<label> <url> <commit> <bin>)..."
        );
        return ExitCode::from(2);
    }
    let (store, git, toolchain, rest) = (&args[0], &args[1], &args[2], &args[3..]);
    let setup = Setup {
        store: PathBuf::from(store),
        git: PathBuf::from(git),
        toolchain: PathBuf::from(toolchain),
    };
    let chains: Vec<Chain> = rest
        .chunks(4)
        .map(|chain| Chain {
            label: chain[0].clone(),
            url: chain[1].clone(),
            commit: chain[2].clone(),
            profile: "release".into(),
            bin: chain[3].clone(),
            path: format!("usr/bin/{}", chain[3]),
        })
        .collect();
    let mut loaded = match load(&graph(&setup, &chains)) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("load: {error}");
            return ExitCode::FAILURE;
        }
    };
    let result = run(&mut loaded, chains.len() * 2, true);
    let faults: Vec<String> = loaded
        .fault_reports()
        .map(|report| format!("{}: {}", report.node().unwrap_or("?"), report.reason()))
        .collect();
    if let Err(stopped) = result {
        eprintln!("run stopped: {stopped:?}");
        for fault in &faults {
            eprintln!("fault: {fault}");
        }
        return ExitCode::FAILURE;
    }
    for chain in &chains {
        match take_tar(&mut loaded, &chain.label)
            .as_ref()
            .and_then(digest_of)
        {
            Some(digest) => println!("{} {digest}", chain.label),
            None => {
                eprintln!("{}: no tar", chain.label);
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}
