//! The command line. Unknown flags, missing values and extra words are
//! errors, never ignored.

use crate::Stop;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const HELP: &str = "\
gaia: build images from product descriptions

usage:
  gaia validate <file.toml>... [--kind product|board|family|component] [--recipes DIR]
  gaia plan  <product.toml> [project flags] [--json]
  gaia build <product.toml> [project flags] [--jobs N] [--config-only | --until STEP]
             [--dev-reuse [--verify]]
  gaia lock  <product.toml> [--update SOURCE]... [--check]  (see gaia lock --help)
  gaia clean [--store DIR] [--keep N] [--dry-run]
  gaia store verify [--store DIR] [--fix]
  gaia show  <image>.<output> [--store DIR]
  gaia sizes [<run.json>] [--store DIR] [--json] [--xz]
  gaia sizes --image <disk image[.xz]> [--json]
  gaia sizes --rootfs <erofs image> [--tools DIR] [--store DIR] [--json]

project flags:
  --store DIR        the content store (default: $GAIA_STORE, or
                     $XDG_CACHE_HOME/gaia/store, or ~/.cache/gaia/store)
  --lock FILE        the lock (default: gaia.lock next to the product)
  --recipes DIR      the recipes (default: recipes/ next to the product)
  --tool NAME=PATH   a tool a step takes by input name (git, curl, bwrap,
                     toolchain, ...); repeatable. Without it, a known tool
                     is looked up on PATH, and `toolchain` is rustc's sysroot.
  --input NAME=PATH  a file a component installs as `{ input = NAME }`, such
                     as a developer's key, which is not committed; repeatable.
                     A relative PATH is from the current directory. Every
                     input a component names must be given, and each given
                     one must be used. No `~` expansion: the shell does it.
  --setting IMAGE.NAME=VALUE
                     a recipe parameter of the image IMAGE that the product
                     leaves unset, such as a version known only at build
                     time; repeatable. VALUE is read as the parameter's type
                     (a list of strings: comma-separated). A parameter the
                     product sets, one the recipe does not declare and an
                     image the product does not have are refused.

dev builds (bench iteration only; never a release):
  --dev-reuse        each Buildroot step keeps its output directory under
                     <store>/dev-buildroot and rebuilds only the packages a
                     config change touches, and what depends on them. Its
                     results are keyed apart from clean builds, and a clean
                     build never uses them. Steps that sign with a key,
                     publish or bundle with a signing key are refused. Every
                     printed output gets <output>.gaia-build.json beside it,
                     {\"build\":\"dev-reuse\"}, and the run record says so.
  --verify           after the dev build, build the same product clean and
                     compare, file by file, every output and what each
                     Buildroot step ships. Run it before trusting a dev image
                     for anything but the bench. Two clean builds are equal
                     in all of it: any difference is a stale reuse, and the
                     build exits non-zero.

validate checks description files and what they name: no lock, no tools,
no inputs, no network. It stops at the first bad file, naming the key. Without --kind,
a file with `images` is a product; one with a board's keys (target, kernel,
partitions, ...) is a board, or a family when it has no name and extends
nothing; any other is a component. A product's images must name recipes
that exist: the names are read from --recipes DIR, else recipes/ next to
the product, and the check is skipped when there is no such directory and
no --recipes. Each image's settings are checked against its recipe's
parameters: a name the recipe does not declare, or a value of the wrong
type, is refused. A parameter with no default may be left unset there and
given with plan's or build's --setting.

plan prints the graph document and the plan and runs nothing. build runs
the graph; Ctrl-C stops at the next step boundary. clean keeps what the
latest N builds (default 3) used and removes the rest. show prints what an
output of the latest build holds and the steps it came from. store verify
reads every object in the store in full and checks it against its digest:
it lists each one that does not match, and exits 2 when any does. With
--fix, those are removed, so the next build makes them again.

environment:
  GAIA_EROFS_TOOL=buildroot  an image with an OS makes its EROFS with its
                     build's own host erofs-utils, not the build machine's
                     mkfs.erofs, so a release can pin that tool. Unset or
                     `machine` (the default): the machine's mkfs.erofs when it
                     is erofs-utils 1.8 or later with what the image asks.
                     Plan says which tool each image uses and why.
";

/// `gaia validate`: the files, and the kind to check each as.
#[derive(Debug, Default, PartialEq)]
pub struct Validate {
    pub files: Vec<PathBuf>,
    pub kind: Option<gaia_describe::Kind>,
    pub recipes: Option<PathBuf>,
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Help,
    Version,
    Validate(Validate),
    Plan(Project),
    Build(Project),
    Clean(Clean),
    Store(StoreCommand),
    Show(Show),
    Sizes(Sizes),
    /// `gaia lock`: the words after `lock`.
    Lock(Vec<String>),
}

/// What `plan` and `build` read.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Project {
    pub product: PathBuf,
    pub store: Option<PathBuf>,
    pub lock: Option<PathBuf>,
    pub recipes: Option<PathBuf>,
    pub tools: BTreeMap<String, String>,
    /// `--input NAME=PATH`, by name.
    pub inputs: BTreeMap<String, PathBuf>,
    /// `--setting IMAGE.NAME=VALUE`, in the order given.
    pub settings: Vec<Setting>,
    pub jobs: Option<usize>,
    /// `build` only: run only as far as the steps of this id.
    pub until: Option<String>,
    /// `build` only: resolve and check each image's OS configuration and
    /// make nothing.
    pub config_only: bool,
    /// `build` only: dev reuse of Buildroot's output directories.
    pub dev_reuse: bool,
    /// `build` only, with `dev_reuse`: build clean too and compare.
    pub verify: bool,
    /// `plan` only: the graph and plan as JSON.
    pub json: bool,
}

/// `--setting IMAGE.NAME=VALUE`: a recipe parameter for one image, as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub image: String,
    pub name: String,
    pub value: String,
}

impl Setting {
    /// The flag as it was given.
    pub fn given(&self) -> String {
        format!("--setting {}.{}={}", self.image, self.name, self.value)
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Clean {
    pub store: Option<PathBuf>,
    pub keep: Option<usize>,
    pub dry_run: bool,
}

/// `gaia store <action>`.
#[derive(Debug, PartialEq)]
pub enum StoreCommand {
    Verify(Verify),
}

/// `gaia store verify`: every object read in full (see `Store::verify`).
#[derive(Debug, Default, PartialEq)]
pub struct Verify {
    pub store: Option<PathBuf>,
    pub fix: bool,
}

#[derive(Debug, Default, PartialEq)]
pub struct Sizes {
    /// The run's record; the latest in the store when not given.
    pub run: Option<PathBuf>,
    pub store: Option<PathBuf>,
    pub json: bool,
    /// Also compress each disk image, for its size.
    pub xz: bool,
    /// A disk image to measure in place of a run: one this builder did
    /// not make.
    pub image: Option<PathBuf>,
    /// An erofs root filesystem image to measure in place of a run.
    pub rootfs: Option<PathBuf>,
    /// With `rootfs`: a build's host tools (the directory with
    /// `dump.erofs` and `fsck.erofs`, or one whose `bin` or `host/bin`
    /// has them); the latest build's in the store when not given.
    pub tools: Option<PathBuf>,
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
                    "--input" => {
                        let pair = words.value(word)?;
                        let (name, path) = pair
                            .split_once('=')
                            .filter(|(n, p)| !n.is_empty() && !p.is_empty())
                            .ok_or_else(|| {
                                Stop::usage(format!("--input takes NAME=PATH, not `{pair}`"))
                            })?;
                        if project.inputs.insert(name.into(), path.into()).is_some() {
                            return Err(Stop::usage(format!("--input {name} is given twice")));
                        }
                    }
                    "--setting" => {
                        let text = words.value(word)?;
                        let setting = text
                            .split_once('=')
                            .and_then(|(at, value)| {
                                let (image, name) = at.rsplit_once('.')?;
                                (!image.is_empty() && !name.is_empty()).then(|| Setting {
                                    image: image.into(),
                                    name: name.into(),
                                    value: value.into(),
                                })
                            })
                            .ok_or_else(|| {
                                Stop::usage(format!(
                                    "--setting takes IMAGE.NAME=VALUE, not `{text}`"
                                ))
                            })?;
                        if project
                            .settings
                            .iter()
                            .any(|s| s.image == setting.image && s.name == setting.name)
                        {
                            return Err(Stop::usage(format!(
                                "--setting {}.{} is given twice",
                                setting.image, setting.name
                            )));
                        }
                        project.settings.push(setting);
                    }
                    "--json" if !build => project.json = true,
                    "--jobs" if build => {
                        let text = words.value(word)?;
                        let jobs = text.parse().ok().filter(|&n: &usize| n > 0);
                        project.jobs = Some(jobs.ok_or_else(|| {
                            Stop::usage(format!("--jobs takes a count above 0, not `{text}`"))
                        })?);
                    }
                    "--until" if build => project.until = Some(words.value(word)?.to_string()),
                    "--config-only" if build => project.config_only = true,
                    "--dev-reuse" if build => project.dev_reuse = true,
                    "--verify" if build => project.verify = true,
                    _ => words.positional(word, &mut product)?,
                }
            }
            project.product = product
                .ok_or_else(|| Stop::usage(format!("{first} needs a product.toml")))?
                .into();
            if project.verify && !project.dev_reuse {
                return Err(Stop::usage(
                    "--verify checks a dev build: it needs --dev-reuse",
                ));
            }
            if build {
                Command::Build(project)
            } else {
                Command::Plan(project)
            }
        }
        "validate" => {
            let mut validate = Validate::default();
            while let Some(word) = words.next() {
                match word {
                    "--kind" => {
                        let text = words.value(word)?;
                        validate.kind =
                            Some(gaia_describe::Kind::parse(text).ok_or_else(|| {
                                Stop::usage(format!(
                                    "--kind takes product, board, family or component, not `{text}`"
                                ))
                            })?);
                    }
                    "--recipes" => validate.recipes = Some(words.value(word)?.into()),
                    flag if flag.starts_with('-') => {
                        return Err(Stop::usage(format!("validate has no flag `{flag}`")));
                    }
                    file => validate.files.push(file.into()),
                }
            }
            if validate.files.is_empty() {
                return Err(Stop::usage("validate needs at least one file"));
            }
            Command::Validate(validate)
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
        "store" => {
            let action = words
                .next()
                .ok_or_else(|| Stop::usage("store needs an action: verify"))?;
            match action {
                "verify" => {
                    let mut verify = Verify::default();
                    let mut none = None;
                    while let Some(word) = words.next() {
                        match word {
                            "--store" => verify.store = Some(words.value(word)?.into()),
                            "--fix" => verify.fix = true,
                            _ => words.positional(word, &mut none)?,
                        }
                    }
                    if let Some(extra) = none {
                        return Err(Stop::usage(format!("store verify takes no `{extra}`")));
                    }
                    Command::Store(StoreCommand::Verify(verify))
                }
                other => return Err(Stop::usage(format!("store has no action `{other}`"))),
            }
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
        "sizes" => {
            let mut sizes = Sizes::default();
            let mut run = None;
            while let Some(word) = words.next() {
                match word {
                    "--store" => sizes.store = Some(words.value(word)?.into()),
                    "--json" => sizes.json = true,
                    "--xz" => sizes.xz = true,
                    "--image" => sizes.image = Some(words.value(word)?.into()),
                    "--rootfs" => sizes.rootfs = Some(words.value(word)?.into()),
                    "--tools" => sizes.tools = Some(words.value(word)?.into()),
                    _ => words.positional(word, &mut run)?,
                }
            }
            sizes.run = run.map(PathBuf::from);
            Command::Sizes(sizes)
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
    fn store_verify_takes_a_store_and_fix() {
        let Ok(Command::Store(StoreCommand::Verify(verify))) =
            parsed("store verify --fix --store /x")
        else {
            panic!("store verify");
        };
        assert!(verify.fix);
        assert_eq!(verify.store, Some(PathBuf::from("/x")));
        assert_eq!(
            parsed("store").expect_err("an error"),
            "store needs an action: verify"
        );
        assert_eq!(
            parsed("store scrub").expect_err("an error"),
            "store has no action `scrub`"
        );
        assert_eq!(
            parsed("store verify extra").expect_err("an error"),
            "store verify takes no `extra`"
        );
    }

    #[test]
    fn flags_and_errors() {
        let Ok(Command::Build(p)) = parsed("build p.toml --jobs 4 --tool git=/usr/bin/git") else {
            panic!("build");
        };
        assert_eq!(p.product, PathBuf::from("p.toml"));
        assert_eq!(p.jobs, Some(4));
        assert_eq!(p.tools["git"], "/usr/bin/git");
        let Ok(Command::Plan(p)) = parsed("plan p.toml --input bench-key=keys/a.pub") else {
            panic!("plan");
        };
        assert_eq!(p.inputs["bench-key"], PathBuf::from("keys/a.pub"));
        assert_eq!(
            parsed("plan p.toml --input bench-key").expect_err("an error"),
            "--input takes NAME=PATH, not `bench-key`"
        );
        assert_eq!(
            parsed("plan p.toml --input =keys/a.pub").expect_err("an error"),
            "--input takes NAME=PATH, not `=keys/a.pub`"
        );
        assert_eq!(
            parsed("plan p.toml --input a=x --input a=y").expect_err("an error"),
            "--input a is given twice"
        );
        let Ok(Command::Build(p)) =
            parsed("build p.toml --setting demo.version=2026.1.0 --setting a.b.c=x=y")
        else {
            panic!("build");
        };
        assert_eq!(
            p.settings,
            [
                Setting {
                    image: "demo".into(),
                    name: "version".into(),
                    value: "2026.1.0".into()
                },
                Setting {
                    image: "a.b".into(),
                    name: "c".into(),
                    value: "x=y".into()
                }
            ]
        );
        assert_eq!(p.settings[1].given(), "--setting a.b.c=x=y");
        for bad in ["version=1", ".version=1", "a.=1", "a.version"] {
            assert_eq!(
                parsed(&format!("plan p.toml --setting {bad}")).expect_err("an error"),
                format!("--setting takes IMAGE.NAME=VALUE, not `{bad}`")
            );
        }
        assert_eq!(
            parsed("plan p.toml --setting a.v=1 --setting a.v=2").expect_err("an error"),
            "--setting a.v is given twice"
        );
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
        assert!(matches!(
            parsed("validate p.toml --recipes r"),
            Ok(Command::Validate(Validate {
                recipes: Some(_),
                ..
            }))
        ));
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
