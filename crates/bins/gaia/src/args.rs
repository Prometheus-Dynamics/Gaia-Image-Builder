//! The command line. Unknown flags, missing values and extra words are
//! errors, never ignored.

use crate::Stop;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const HELP: &str = "\
gaia: build images from product descriptions

usage:
  gaia plan  <product.toml> [project flags] [--json]
  gaia build <product.toml> [project flags] [--jobs N]
  gaia lock  <product.toml> [--update SOURCE]... [--check]  (see gaia lock --help)
  gaia clean [--store DIR] [--keep N] [--dry-run]
  gaia show  <image>.<output> [--store DIR]

project flags:
  --store DIR        the content store (default: $GAIA_STORE, or
                     $XDG_CACHE_HOME/gaia/store, or ~/.cache/gaia/store)
  --lock FILE        the lock (default: gaia.lock next to the product)
  --recipes DIR      the recipes (default: recipes/ next to the product)
  --tool NAME=PATH   a tool a step takes by input name (git, curl, bwrap,
                     toolchain, ...); repeatable. Without it, a known tool
                     is looked up on PATH, and `toolchain` is rustc's sysroot.

plan prints the graph document and the plan and runs nothing. build runs
the graph; Ctrl-C stops at the next step boundary. clean keeps what the
latest N builds (default 3) used and removes the rest. show prints what an
output of the latest build holds and the steps it came from.
";

#[derive(Debug, PartialEq)]
pub enum Command {
    Help,
    Version,
    Plan(Project),
    Build(Project),
    Clean(Clean),
    Show(Show),
    /// `gaia lock`: the words after `lock`.
    Lock(Vec<String>),
}

/// What `plan` and `build` read.
#[derive(Debug, Default, PartialEq)]
pub struct Project {
    pub product: PathBuf,
    pub store: Option<PathBuf>,
    pub lock: Option<PathBuf>,
    pub recipes: Option<PathBuf>,
    pub tools: BTreeMap<String, String>,
    pub jobs: Option<usize>,
    /// `plan` only: the graph and plan as JSON.
    pub json: bool,
}

#[derive(Debug, Default, PartialEq)]
pub struct Clean {
    pub store: Option<PathBuf>,
    pub keep: Option<usize>,
    pub dry_run: bool,
}

#[derive(Debug, Default, PartialEq)]
pub struct Show {
    pub output: String,
    pub store: Option<PathBuf>,
}

pub fn parse(words: &[String]) -> Result<Command, Stop> {
    let Some((first, rest)) = words.split_first() else {
        return Ok(Command::Help);
    };
    let mut words = Words { rest, at: 0 };
    let command = match first.as_str() {
        "-h" | "--help" | "help" => return Ok(Command::Help),
        "-V" | "--version" => return Ok(Command::Version),
        "plan" | "build" => {
            let build = first == "build";
            let mut project = Project::default();
            let mut product = None;
            while let Some(word) = words.next() {
                match word {
                    "--store" => project.store = Some(words.value(word)?.into()),
                    "--lock" => project.lock = Some(words.value(word)?.into()),
                    "--recipes" => project.recipes = Some(words.value(word)?.into()),
                    "--tool" => {
                        let pair = words.value(word)?;
                        let (name, path) = pair
                            .split_once('=')
                            .filter(|(n, p)| !n.is_empty() && !p.is_empty())
                            .ok_or_else(|| {
                                Stop::usage(format!("--tool takes NAME=PATH, not `{pair}`"))
                            })?;
                        project.tools.insert(name.into(), path.into());
                    }
                    "--json" if !build => project.json = true,
                    "--jobs" if build => {
                        let text = words.value(word)?;
                        let jobs = text.parse().ok().filter(|&n: &usize| n > 0);
                        project.jobs = Some(jobs.ok_or_else(|| {
                            Stop::usage(format!("--jobs takes a count above 0, not `{text}`"))
                        })?);
                    }
                    _ => words.positional(word, &mut product)?,
                }
            }
            project.product = product
                .ok_or_else(|| Stop::usage(format!("{first} needs a product.toml")))?
                .into();
            if build {
                Command::Build(project)
            } else {
                Command::Plan(project)
            }
        }
        "lock" => return Ok(Command::Lock(rest.to_vec())),
        "clean" => {
            let mut clean = Clean::default();
            let mut none = None;
            while let Some(word) = words.next() {
                match word {
                    "--store" => clean.store = Some(words.value(word)?.into()),
                    "--dry-run" => clean.dry_run = true,
                    "--keep" => {
                        let text = words.value(word)?;
                        clean.keep = Some(text.parse().map_err(|_| {
                            Stop::usage(format!("--keep takes a count, not `{text}`"))
                        })?);
                    }
                    _ => words.positional(word, &mut none)?,
                }
            }
            if let Some(extra) = none {
                return Err(Stop::usage(format!("clean takes no `{extra}`")));
            }
            Command::Clean(clean)
        }
        "show" => {
            let mut show = Show::default();
            let mut output = None;
            while let Some(word) = words.next() {
                match word {
                    "--store" => show.store = Some(words.value(word)?.into()),
                    _ => words.positional(word, &mut output)?,
                }
            }
            show.output =
                output.ok_or_else(|| Stop::usage("show needs an output, as <image>.<output>"))?;
            Command::Show(show)
        }
        other => {
            return Err(Stop::usage(format!(
                "no command `{other}`; try `gaia --help`"
            )));
        }
    };
    Ok(command)
}

struct Words<'a> {
    rest: &'a [String],
    at: usize,
}

impl<'a> Words<'a> {
    fn next(&mut self) -> Option<&'a str> {
        let word = self.rest.get(self.at)?;
        self.at += 1;
        Some(word)
    }

    fn value(&mut self, flag: &str) -> Result<&'a str, Stop> {
        self.next()
            .filter(|v| !v.starts_with("--"))
            .ok_or_else(|| Stop::usage(format!("{flag} needs a value")))
    }

    /// The one positional word, or an error for a flag or a second word.
    fn positional(&self, word: &str, slot: &mut Option<String>) -> Result<(), Stop> {
        if word.starts_with('-') {
            return Err(Stop::usage(format!("unknown flag `{word}`")));
        }
        if let Some(first) = slot {
            return Err(Stop::usage(format!("`{word}` after `{first}`: one only")));
        }
        *slot = Some(word.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(line: &str) -> Result<Command, String> {
        let words: Vec<String> = line.split_whitespace().map(String::from).collect();
        parse(&words).map_err(|s| s.message)
    }

    #[test]
    fn flags_and_errors() {
        let Ok(Command::Build(p)) = parsed("build p.toml --jobs 4 --tool git=/usr/bin/git") else {
            panic!("build");
        };
        assert_eq!(p.product, PathBuf::from("p.toml"));
        assert_eq!(p.jobs, Some(4));
        assert_eq!(p.tools["git"], "/usr/bin/git");
        assert_eq!(parsed(""), Ok(Command::Help));
        assert_eq!(
            parsed("plan p.toml --jobs 4").expect_err("an error"),
            "unknown flag `--jobs`"
        );
        assert_eq!(
            parsed("plan").expect_err("an error"),
            "plan needs a product.toml"
        );
        assert_eq!(
            parsed("plan a.toml b.toml").expect_err("an error"),
            "`b.toml` after `a.toml`: one only"
        );
        assert_eq!(
            parsed("build p.toml --store").expect_err("an error"),
            "--store needs a value"
        );
        assert_eq!(
            parsed("build p.toml --tool git").expect_err("an error"),
            "--tool takes NAME=PATH, not `git`"
        );
        assert_eq!(
            parsed("clean extra").expect_err("an error"),
            "clean takes no `extra`"
        );
        assert!(matches!(
            parsed("clean --keep 1 --dry-run"),
            Ok(Command::Clean(Clean {
                keep: Some(1),
                dry_run: true,
                ..
            }))
        ));
        assert_eq!(
            parsed("frob").expect_err("an error"),
            "no command `frob`; try `gaia --help`"
        );
    }
}
