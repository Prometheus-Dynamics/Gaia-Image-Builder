//! `gaia-lock <product.toml> [--lock FILE] [--update SOURCE]... [--check]
//! [--tool git=PATH] [--tool curl=PATH]`
//!
//! Exit status: 0 done (or, with `--check`, the lock matches), 1 a source
//! did not resolve (or the lock doesn't match), 2 the command line or a
//! description is wrong.

use gaia_lock_cmd::{Options, Tools, describe, lock, lock_path, wants};
use std::path::PathBuf;
use std::process::ExitCode;

const HELP: &str = "\
gaia lock: resolve every source a product names and write its lock

usage: gaia-lock <product.toml> [--lock FILE] [--update SOURCE]... [--check]
                 [--tool git=PATH] [--tool curl=PATH]

  --lock FILE       the lock (default: gaia.lock next to the product)
  --update SOURCE   resolve SOURCE again (a name such as `kernel.src`)
  --check           write nothing; exit 1 unless the lock matches
  --tool NAME=PATH  the git or curl program (default: found on PATH)
";

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err((code, message)) => {
            eprintln!("gaia lock: {message}");
            ExitCode::from(code)
        }
    }
}

fn run() -> Result<ExitCode, (u8, String)> {
    let usage = |m: String| (2u8, m);
    let mut product = None;
    let mut lock_file = None;
    let mut update = Vec::new();
    let mut check = false;
    let mut git = None;
    let mut curl = None;
    let mut words = std::env::args().skip(1);
    while let Some(word) = words.next() {
        let mut value = |flag: &str| {
            words
                .next()
                .filter(|v| !v.starts_with("--"))
                .ok_or_else(|| usage(format!("{flag} needs a value")))
        };
        match word.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(ExitCode::SUCCESS);
            }
            "--lock" => lock_file = Some(PathBuf::from(value("--lock")?)),
            "--update" => update.push(value("--update")?),
            "--check" => check = true,
            "--tool" => {
                let pair = value("--tool")?;
                match pair.split_once('=') {
                    Some(("git", path)) if !path.is_empty() => git = Some(PathBuf::from(path)),
                    Some(("curl", path)) if !path.is_empty() => curl = Some(PathBuf::from(path)),
                    _ => {
                        return Err(usage(format!(
                            "--tool takes git=PATH or curl=PATH, not `{pair}`"
                        )));
                    }
                }
            }
            flag if flag.starts_with('-') => return Err(usage(format!("unknown flag `{flag}`"))),
            _ if product.is_some() => return Err(usage(format!("`{word}`: one product only"))),
            _ => product = Some(PathBuf::from(word)),
        }
    }
    let product = product.ok_or_else(|| usage("give a product.toml; see --help".into()))?;
    if check && !update.is_empty() {
        return Err(usage("--check writes nothing; drop --update".into()));
    }
    let scratch = std::env::temp_dir().join(format!("gaia-lock-{}", std::process::id()));
    let options = Options {
        product,
        lock: lock_file,
        update,
        tools: Tools {
            git: git
                .or_else(|| on_path("git"))
                .unwrap_or_else(|| "git".into()),
            curl: curl
                .or_else(|| on_path("curl"))
                .unwrap_or_else(|| "curl".into()),
        },
        scratch: scratch.clone(),
    };
    let path = lock_path(&options);

    let wanted = wants(&options.product).map_err(usage)?;
    if let Some(name) = options
        .update
        .iter()
        .find(|name| !wanted.iter().any(|(n, _)| n == *name))
    {
        let names: Vec<&str> = wanted.iter().map(|(n, _)| n.as_str()).collect();
        return Err(usage(format!(
            "no source `{name}`; the descriptions name: {}",
            names.join(", ")
        )));
    }

    if check {
        let lock = gaia_lock::Lock::read(&path).map_err(|e| (1, e.to_string()))?;
        return match lock.check(wanted.iter().map(|(n, w)| (n.as_str(), w))) {
            Ok(()) => {
                println!("{} matches the descriptions", path.display());
                Ok(ExitCode::SUCCESS)
            }
            Err(e) => Err((1, e.to_string())),
        };
    }

    std::fs::create_dir_all(&scratch).map_err(|e| (1, format!("{}: {e}", scratch.display())))?;
    let result = lock(&options);
    let _ = std::fs::remove_dir_all(&scratch);
    let (lock, changes) = result.map_err(|e| (1, e))?;
    for line in describe(&changes) {
        println!("{line}");
    }
    if changes.is_empty() && path.exists() {
        println!("{} is up to date", path.display());
    } else {
        lock.write(&path).map_err(|e| (1, e.to_string()))?;
        println!("wrote {}", path.display());
    }
    Ok(ExitCode::SUCCESS)
}

fn on_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|path| path.is_file())
}
