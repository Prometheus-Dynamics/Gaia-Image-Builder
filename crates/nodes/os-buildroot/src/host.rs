//! `gaia.os.buildroot.host`: a config's host tools, built once.
//!
//! One image's kernel, bootloader and root filesystem are three Buildroot
//! builds, and each would build the same host tools (the fake root, the
//! device tree compiler, a host Python). This step builds them once, from
//! the image's whole config, and each build starts with them in place.
//!
//! What it builds: every package of the config's build order that is a host
//! package and whose dependencies, all the way down, are host packages. A
//! host package that needs a target package (an internal toolchain's final
//! compiler needs the C library) is left to each build, and so is the
//! external toolchain, which also installs into the target.
//!
//! Its output, the *seed*: Buildroot's `host/` directory, and for each
//! package built the files Buildroot marks its steps done with
//! (`build/<package>/.stamp_*` and its file lists). A build with the seed
//! planted in its output directory finds those packages installed.
//!
//! A step's output directory is a new path every time, and host tools name
//! it (a `.pc` file's prefix, a script's interpreter). The seed is made
//! independent of the path the way Buildroot's own SDK is: library search
//! paths become relative (`support/scripts/fix-rpath host`), and in every
//! text file the path is replaced by a marker, listed in `relocs`; planting
//! puts the new path back. Binaries that name the path in their data keep
//! the old one, as in Buildroot's SDK; they are listed in `binaries`.
//!
//! The key is not the whole config: a BusyBox option must not rebuild the
//! host tools. A first step, the *plan*, keyed on the whole config, asks
//! Buildroot what it would do: the packages to build, and the expanded value
//! of every variable of those packages and of the host compiler's flags
//! (`make printvars`), with the paths of this run replaced by names. The
//! build is keyed on the Buildroot source, the packages it builds, a digest
//! of their values, and the input trees those values name. A package that
//! names a board's input tree, or a source the config overrides, is built by
//! each build instead of the seed, so it is not in that key. The reverse
//! dependency lists are not in it either: they name the targets a config
//! selects, and no host tool is built differently for them. So what changes
//! how a shared host tool is built changes the key, and nothing else does.

use crate::{BuildrootHost, OsConfig, Prepared, configure, make_tool, prepare};
use daedalus::prelude::*;
use daedalus_cpu_author::FanIn;
use gaia_step::{Ctx, Digest, KeyBuilder, Step, StepError, Store, Stream, Tool, Work};
use gaia_types::{SourceOverride, Tree};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const PLAN: &str = "gaia.os.buildroot.host-plan";
const NAME: &str = "gaia.os.buildroot.host";
const KEY_VERSION: u32 = 3;
/// What stands for the output directory in the seed's text files.
const MARKER: &str = "@GAIA_BUILDROOT_OUT@";
/// Variables every host package is built with, beside its own.
const SHARED_VARS: &str = "HOST_CFLAGS HOST_CPPFLAGS HOST_CXXFLAGS HOST_LDFLAGS HOST_MAKE_ENV \
    HOST_CONFIGURE_OPTS HOSTCC HOSTCXX";
/// The modification time planted stamps get: one time for all, so none is
/// older than the one make checks it against.
const STAMP_SECONDS: u64 = 1_000_000_000;

/// The host tools of the Buildroot tree `source` under `os` and its input
/// trees, as a seed for `gaia.os.buildroot`.
#[node(
    id = "gaia.os.buildroot.host",
    inputs(store, source, os, inputs, host),
    outputs(seed)
)]
pub fn os_buildroot_host(
    store: &str,
    source: Tree,
    os: &OsConfig,
    inputs: FanIn<'_, Tree>,
    host: &BuildrootHost,
    cx: &Ctx,
) -> Result<Tree, StepError> {
    let inputs: Vec<Tree> = inputs.iter().map(|tree| (*tree).clone()).collect();
    build_host(store, source, os, &inputs, host, cx)
}

/// The node's body, with the input trees as a slice.
pub fn build_host(
    store: &str,
    source: Tree,
    config: &OsConfig,
    inputs: &[Tree],
    host: &BuildrootHost,
    cx: &Ctx,
) -> Result<Tree, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let prepared =
        prepare(&store, &source, config, inputs, host).map_err(|why| StepError::body(NAME, why))?;

    // What Buildroot would build for the host, and with what.
    let plan_key = prepared.key(PLAN, KEY_VERSION, config, host).finish();
    let planned = Step::new(&store, cx, PLAN, plan_key)
        .look_up_or_make(&[("plan", Path::new("plan.txt"))], |work| {
            plan(work, &prepared, host)
        })?;
    let plan_text = fs::read_to_string(store.path(&planned.outputs["plan"]))
        .map_err(|e| StepError::body(PLAN, format!("the plan: {e}")))?;
    // Only what the shared seed is built from is keyed: see `shared_plan`.
    let shared = shared_plan(&plan_text, &config.inputs, &config.overrides);
    let (goals, text) = (shared.goals.join(" "), shared.text);
    // A check of the configuration alone: it is resolved and held, and no
    // tool is made. The seed is an empty tree nothing may build from.
    if host.config_only {
        let empty = store.scratch().map_err(StepError::store(NAME))?;
        let dir = empty.path().join("empty");
        fs::create_dir(&dir).map_err(|e| StepError::body(NAME, e.to_string()))?;
        let object = store.put_tree(&dir).map_err(StepError::store(NAME))?;
        return Ok(Tree {
            object: object.into(),
            entries: Vec::new(),
        });
    }

    let mut key = KeyBuilder::new(NAME, KEY_VERSION)
        .input("source", prepared.src_digest)
        .config("make", &host.make)
        .config("goals", &goals)
        .input("values", Digest::of_bytes(text.as_bytes()));
    // An input tree the values name is one a host package is built from.
    for (name, _, digest) in &prepared.dirs {
        if text.contains(&format!("@{name}@")) {
            key = key.input(&format!("input.{name}"), *digest);
        }
    }
    let goals: Vec<&str> = goals.split_whitespace().collect();
    let made = Step::new(&store, cx, NAME, key.finish())
        .look_up_or_make(&[("seed", Path::new("seed"))], |work| {
            build(work, &prepared, &goals, host)
        })?;
    Ok(Tree {
        object: made.outputs["seed"].into(),
        entries: Vec::new(),
    })
}

/// One make run whose standard output is kept.
fn captured(work: &mut Work<'_>, tool: Tool) -> Result<String, StepError> {
    let shown = Arc::new(Mutex::new(String::new()));
    let sink = shown.clone();
    work.run(&tool.on_line(move |stream, line| {
        if stream == Stream::Stdout
            && let Ok(mut text) = sink.lock()
        {
            text.push_str(line);
            text.push('\n');
        }
    }))?;
    Ok(shown.lock().map(|text| text.clone()).unwrap_or_default())
}

/// The host packages of Buildroot's `show-info` (a JSON object, one entry
/// per package with its `dependencies`) that depend, all the way down, on
/// host packages only. Sorted.
pub fn host_goals(show_info: &str) -> Result<Vec<String>, String> {
    let info: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(show_info).map_err(|e| format!("Buildroot's show-info: {e}"))?;
    fn all_host<'a>(
        name: &'a str,
        info: &'a BTreeMap<String, serde_json::Value>,
        known: &mut BTreeMap<&'a str, bool>,
    ) -> bool {
        if let Some(&answer) = known.get(name) {
            return answer;
        }
        let Some(entry) = info.get(name).filter(|_| name.starts_with("host-")) else {
            known.insert(name, false);
            return false;
        };
        // A cycle cannot be all host packages by this rule; none exists.
        known.insert(name, false);
        let answer = entry["dependencies"]
            .as_array()
            .map(|deps| {
                deps.iter()
                    .all(|dep| dep.as_str().is_some_and(|dep| all_host(dep, info, known)))
            })
            .unwrap_or(true);
        known.insert(name, answer);
        answer
    }
    let mut known = BTreeMap::new();
    Ok(info
        .keys()
        .filter(|name| all_host(name, &info, &mut known))
        .cloned()
        .collect())
}

/// The plan as the shared seed sees it.
pub struct SharedPlan {
    /// The goals the shared seed builds, in plan order.
    pub goals: Vec<String>,
    /// The goal line, then the variables of the shared goals and the global
    /// ones. Reverse dependency variables are left out.
    pub text: String,
}

/// Splits a plan into the goals of the shared seed and the values they are
/// keyed on. A goal is left to each build when it names an input tree (the
/// board's own files), when it is the host variant of a source the config
/// overrides (`host-uboot-tools` for `UBOOT`), or when it depends on such a
/// goal, all the way down.
pub fn shared_plan(plan: &str, inputs: &[String], overrides: &[SourceOverride]) -> SharedPlan {
    let (goal_line, body) = plan.split_once('\n').unwrap_or((plan, ""));
    let goals: Vec<&str> = goal_line.split_whitespace().collect();
    let vars = variables(body);
    let values_of = |goal: &str| -> String {
        vars.iter()
            .filter(|(name, _)| owner(name, &goals) == Some(goal))
            .map(|(_, lines)| lines.as_str())
            .collect()
    };
    let mut board: BTreeSet<&str> = BTreeSet::new();
    for goal in &goals {
        let own = values_of(goal);
        let named = inputs
            .iter()
            .any(|input| own.contains(&format!("@{input}@")));
        if named || overrides.iter().any(|o| overrides_goal(&o.package, goal)) {
            board.insert(*goal);
        }
    }
    let mut grew = true;
    while grew {
        grew = false;
        for goal in &goals {
            if !board.contains(goal) && depends_on_board(&vars, goal, &board) {
                board.insert(*goal);
                grew = true;
            }
        }
    }
    let shared: Vec<&str> = goals
        .iter()
        .copied()
        .filter(|goal| !board.contains(goal))
        .collect();
    let mut text = format!("{}\n", shared.join(" "));
    for (name, lines) in &vars {
        if name.ends_with("_RDEPENDENCIES")
            || owner(name, &goals).is_some_and(|g| board.contains(g))
        {
            continue;
        }
        text.push_str(lines);
    }
    SharedPlan {
        goals: shared.iter().map(|goal| goal.to_string()).collect(),
        text,
    }
}

/// Whether a goal's `DEPENDENCIES` name one of the board's goals.
fn depends_on_board(vars: &[(&str, String)], goal: &str, board: &BTreeSet<&str>) -> bool {
    let want = format!("{}DEPENDENCIES", variable_prefix(goal));
    vars.iter()
        .find(|(name, _)| *name == want)
        .and_then(|(_, lines)| lines.lines().next())
        .and_then(|line| line.split_once('='))
        .is_some_and(|(_, deps)| deps.split_whitespace().any(|dep| board.contains(dep)))
}

/// The variables of a plan body: each `NAME=` line with the lines after it,
/// up to the next one, since a value may span lines.
fn variables(body: &str) -> Vec<(&str, String)> {
    let mut found: Vec<(&str, String)> = Vec::new();
    for line in body.lines() {
        if let Some(name) = variable_name(line) {
            found.push((name, format!("{line}\n")));
        } else if let Some((_, lines)) = found.last_mut() {
            lines.push_str(line);
            lines.push('\n');
        }
    }
    found
}

/// The name a line sets: upper-case letters, digits and underscores, then `=`.
fn variable_name(line: &str) -> Option<&str> {
    let (name, _) = line.split_once('=')?;
    let valid = name.starts_with(|c: char| c.is_ascii_uppercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
    valid.then_some(name)
}

/// The start of a host package's variable names: `host-acl` has `HOST_ACL_`.
fn variable_prefix(goal: &str) -> String {
    let name = goal.strip_prefix("host-").unwrap_or(goal);
    format!("HOST_{}_", name.to_uppercase().replace('-', "_"))
}

/// The goal a variable belongs to: the longest goal whose prefix the name
/// has, so `HOST_GETTEXT_TINY_VERSION` is not a variable of `host-gettext`.
fn owner<'a>(name: &str, goals: &[&'a str]) -> Option<&'a str> {
    goals
        .iter()
        .copied()
        .filter(|goal| name.starts_with(variable_prefix(goal).as_str()))
        .max_by_key(|goal| goal.len())
}

/// Whether the config overrides a source the host package is built from:
/// `UBOOT` covers `host-uboot` and `host-uboot-tools`.
fn overrides_goal(package: &str, goal: &str) -> bool {
    let package = package.to_lowercase().replace('_', "-");
    let name = goal.strip_prefix("host-").unwrap_or(goal);
    name == package || name.starts_with(&format!("{package}-"))
}

/// The plan: the goals on the first line, then the values they are built
/// with, with this run's paths replaced by names.
fn plan(work: &mut Work<'_>, prepared: &Prepared, host: &BuildrootHost) -> Result<(), StepError> {
    let fail = |why: String| StepError::body(PLAN, why);
    let dir = work.dir().to_path_buf();
    let out = dir.join("out");
    fs::create_dir(&out).map_err(|e| fail(format!("output directory: {e}")))?;
    configure(PLAN, work, prepared, &out, host)?;
    let (src, vars) = (&prepared.src, &prepared.vars);
    let info = captured(work, make_tool(host, src, &out, vars, &["show-info"]))?;
    let goals = host_goals(&info).map_err(fail)?;
    let mut text = goals.join(" ");
    text.push('\n');
    if !goals.is_empty() {
        let mut patterns: Vec<String> = goals
            .iter()
            .map(|goal| format!("{}_%", goal.to_uppercase().replace(['-', '.'], "_")))
            .collect();
        patterns.push(SHARED_VARS.into());
        // One job, so the job count of this machine is in no value.
        let tool = make_tool(host, src, &out, vars, &["printvars"])
            .arg(format!("VARS={}", patterns.join(" ")))
            .arg("PARALLEL_JOBS=1");
        let mut values = captured(work, tool)?;
        let mut names: Vec<(String, String)> = vec![
            (out.display().to_string(), "@out@".into()),
            (src.display().to_string(), "@buildroot@".into()),
            (host.download_dir.clone(), "@downloads@".into()),
        ];
        for (name, dir, _) in &prepared.dirs {
            names.push((dir.display().to_string(), format!("@{name}@")));
        }
        // Longest first: one path may begin another.
        names.sort_by_key(|(path, _)| std::cmp::Reverse(path.len()));
        for (path, name) in &names {
            values = values.replace(path, name);
        }
        text.push_str(&values);
    }
    fs::write(dir.join("plan.txt"), text).map_err(|e| fail(format!("the plan: {e}")))
}

/// Builds the goals and makes the seed of what they installed.
fn build(
    work: &mut Work<'_>,
    prepared: &Prepared,
    goals: &[&str],
    host: &BuildrootHost,
) -> Result<(), StepError> {
    // One build at a time: each has the whole job count.
    let _turn = crate::turn();
    let io_fail = |what: &'static str| {
        move |error: io::Error| StepError::body(NAME, format!("{what}: {error}"))
    };
    let dir = work.dir().to_path_buf();
    let seed = dir.join("seed");
    fs::create_dir(&seed).map_err(io_fail("the seed"))?;
    if goals.is_empty() {
        return Ok(());
    }
    let out = dir.join("out");
    fs::create_dir(&out).map_err(io_fail("output directory"))?;
    configure(NAME, work, prepared, &out, host)?;
    let (src, vars) = (&prepared.src, &prepared.vars);
    work.run(&make_tool(host, src, &out, vars, goals))?;
    // Library search paths relative to the binary, as Buildroot does at the
    // end of a whole build and for its SDK.
    if out.join("host/bin/patchelf").is_file() {
        let fix = Tool::new(src.join("support/scripts/fix-rpath").display().to_string())
            .arg("host")
            .cwd(src)
            .env("PATH", crate::PATH)
            .env("LC_ALL", "C")
            .env("HOST_DIR", out.join("host").display().to_string())
            .env("STAGING_DIR", out.join("staging").display().to_string())
            .env("TARGET_DIR", out.join("target").display().to_string())
            .env(
                "PER_PACKAGE_DIR",
                out.join("per-package").display().to_string(),
            )
            .env("PARALLEL_JOBS", host.jobs.to_string());
        work.run(&fix)?;
    }
    fs::rename(out.join("host"), seed.join("host")).map_err(io_fail("the host directory"))?;
    // Each built package's marks: the dot files at the top of its build
    // directory, and nothing of what it compiled.
    let build = seed.join("build");
    fs::create_dir(&build).map_err(io_fail("the seed"))?;
    let mut kept = BTreeSet::new();
    for entry in fs::read_dir(out.join("build")).map_err(io_fail("the build directory"))? {
        let entry = entry.map_err(io_fail("the build directory"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if name == "packages-file-list-host.txt" {
            fs::copy(&path, build.join(&name)).map_err(io_fail("the host file list"))?;
            continue;
        }
        if !name.starts_with("host-") || !path.is_dir() {
            continue;
        }
        fs::create_dir(build.join(&name)).map_err(io_fail("the seed"))?;
        for file in fs::read_dir(&path).map_err(io_fail("a package's marks"))? {
            let file = file.map_err(io_fail("a package's marks"))?;
            let mark = file.file_name().to_string_lossy().into_owned();
            if mark.starts_with('.') && file.path().is_file() {
                fs::copy(file.path(), build.join(&name).join(&mark))
                    .map_err(io_fail("a package's marks"))?;
                kept.insert(format!("{name}/{mark}"));
            }
        }
    }
    let (relocs, links, binaries) =
        unplant(&seed, &out.display().to_string()).map_err(io_fail("the seed's paths"))?;
    fs::write(seed.join("relocs"), relocs.join("\n")).map_err(io_fail("the seed"))?;
    fs::write(seed.join("links"), links.join("\n")).map_err(io_fail("the seed"))?;
    fs::write(seed.join("binaries"), binaries.join("\n")).map_err(io_fail("the seed"))?;
    Ok(())
}

/// Every file under `root`, relative, sorted: regular files and symlinks.
fn files(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() || kind.is_symlink() {
                found.push(
                    entry
                        .path()
                        .strip_prefix(root)
                        .unwrap_or(&entry.path())
                        .to_path_buf(),
                );
            }
        }
    }
    found.sort();
    Ok(found)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Takes the path `out` out of the seed: text files get the marker (and are
/// listed), symlinks whose target names it are removed (and listed as
/// `path<TAB>target` with the marker), and binaries that name it are listed
/// and left as they are.
fn unplant(seed: &Path, out: &str) -> io::Result<(Vec<String>, Vec<String>, Vec<String>)> {
    let (mut relocs, mut links, mut binaries) = (Vec::new(), Vec::new(), Vec::new());
    for relative in files(seed)? {
        let path = seed.join(&relative);
        let shown = relative.display().to_string();
        let meta = fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&path)?.display().to_string();
            if target.contains(out) {
                fs::remove_file(&path)?;
                links.push(format!("{shown}\t{}", target.replace(out, MARKER)));
            }
            continue;
        }
        let bytes = fs::read(&path)?;
        if !contains(&bytes, out.as_bytes()) {
            continue;
        }
        match String::from_utf8(bytes) {
            Ok(text) if !text.contains('\0') => {
                write_keeping_mode(&path, &meta, text.replace(out, MARKER).as_bytes())?;
                relocs.push(shown);
            }
            _ => binaries.push(shown),
        }
    }
    Ok((relocs, links, binaries))
}

/// Replaces a file's content, whatever its mode, and puts the mode back.
fn write_keeping_mode(path: &Path, meta: &fs::Metadata, bytes: &[u8]) -> io::Result<()> {
    let mode = meta.permissions().mode();
    fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o200))?;
    fs::write(path, bytes)?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

/// Plants the seed in the output directory `out`, before Buildroot runs
/// there: its `host/` and package marks, with `out`'s own path where the
/// marker is. An empty seed plants nothing.
pub(crate) fn plant(seed: &Path, out: &Path) -> io::Result<()> {
    if !seed.join("host").is_dir() {
        return Ok(());
    }
    let here = out.display().to_string();
    for part in ["host", "build"] {
        crate::out::copy_tree(&seed.join(part), &out.join(part))?;
    }
    let list = |name: &str| -> io::Result<Vec<String>> {
        Ok(fs::read_to_string(seed.join(name))?
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    };
    for relative in list("relocs")? {
        let path = out.join(&relative);
        let meta = fs::symlink_metadata(&path)?;
        let text = fs::read_to_string(&path)?;
        write_keeping_mode(&path, &meta, text.replace(MARKER, &here).as_bytes())?;
    }
    for line in list("links")? {
        if let Some((relative, target)) = line.split_once('\t') {
            std::os::unix::fs::symlink(target.replace(MARKER, &here), out.join(relative))?;
        }
    }
    // One time for every mark: make compares a package's marks with each
    // other, and a copy gives them the order they were copied in.
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(STAMP_SECONDS);
    for relative in files(&out.join("build"))? {
        let path = out.join("build").join(relative);
        if path.is_file() && !fs::symlink_metadata(&path)?.file_type().is_symlink() {
            let mode = fs::metadata(&path)?.permissions().mode();
            fs::set_permissions(&path, fs::Permissions::from_mode(mode | 0o200))?;
            fs::File::options()
                .write(true)
                .open(&path)?
                .set_modified(time)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plan lines cut from a real plan: four host packages, the global
    /// variables, and two more host packages (`host-libxcrypt` and
    /// `host-mkpasswd`) that only the first plan builds.
    const WITH_EXTRA: &str = include_str!("../tests/host-plan/plan-extra-tools.txt");
    /// The same four packages from a plan that lacks the two extras.
    const BASE: &str = include_str!("../tests/host-plan/plan-base.txt");

    fn no_inputs() -> Vec<String> {
        Vec::new()
    }

    /// The text without its goal line, and without the variables of the
    /// two packages only one plan builds.
    fn values_of_common(text: &str) -> Vec<&str> {
        text.lines()
            .skip(1)
            .filter(|line| {
                !line.starts_with("HOST_MKPASSWD_") && !line.starts_with("HOST_LIBXCRYPT_")
            })
            .collect()
    }

    #[test]
    fn reverse_dependencies_are_not_keyed() {
        let shared = shared_plan(BASE, &no_inputs(), &[]);
        assert_eq!(
            shared.goals,
            ["host-acl", "host-dtc", "host-pkgconf", "host-skeleton"]
        );
        assert!(
            shared
                .text
                .lines()
                .all(|line| !line.contains("_RDEPENDENCIES=")),
            "a reverse dependency list is in the key"
        );
    }

    /// The two plans differ only in their reverse dependency lists and in
    /// the two extra packages. The values the shared seed is keyed on are
    /// the same otherwise, so the two plans share one seed.
    #[test]
    fn same_packages_key_the_same_whatever_the_targets() {
        let with_extra = shared_plan(WITH_EXTRA, &no_inputs(), &[]);
        let base = shared_plan(BASE, &no_inputs(), &[]);
        assert_eq!(
            values_of_common(&with_extra.text),
            values_of_common(&base.text)
        );
        assert_eq!(
            with_extra.text.lines().next(),
            Some(with_extra.goals.join(" ").as_str())
        );
    }

    /// A goal whose values name an input tree is left to each build, and
    /// so no input tree it names is in the shared key.
    #[test]
    fn a_goal_naming_an_input_tree_is_left_to_each_build() {
        let plan = format!("{BASE}HOST_DTC_SOURCE_SITE=@board-tree@/dtc\n");
        let inputs = vec!["board-tree".to_string()];
        let shared = shared_plan(&plan, &inputs, &[]);
        assert_eq!(shared.goals, ["host-acl", "host-pkgconf", "host-skeleton"]);
        assert!(!shared.text.contains("@board-tree@"));
        assert!(!shared.text.contains("HOST_DTC_"));
    }

    #[test]
    fn an_overridden_source_and_what_depends_on_it_are_left_to_each_build() {
        let overrides = vec![SourceOverride {
            package: "PKGCONF".into(),
            input: "board-tree".into(),
        }];
        let shared = shared_plan(BASE, &no_inputs(), &overrides);
        // host-dtc depends on host-pkgconf, so it is left to each build too.
        assert_eq!(shared.goals, ["host-acl", "host-skeleton"]);
        assert!(!shared.text.contains("HOST_DTC_"));
        assert!(!shared.text.contains("HOST_PKGCONF_"));
    }

    #[test]
    fn a_host_variant_of_an_overridden_source_is_board_specific() {
        assert!(overrides_goal("UBOOT", "host-uboot"));
        assert!(overrides_goal("UBOOT", "host-uboot-tools"));
        assert!(!overrides_goal("UBOOT", "host-ubootstrap"));
        assert!(!overrides_goal("UBOOT", "host-acl"));
    }

    #[test]
    fn the_longest_goal_owns_a_variable() {
        let goals = ["host-gettext", "host-gettext-tiny"];
        assert_eq!(
            owner("HOST_GETTEXT_TINY_VERSION", &goals),
            Some("host-gettext-tiny")
        );
        assert_eq!(owner("HOST_GETTEXT_VERSION", &goals), Some("host-gettext"));
        assert_eq!(owner("HOSTCC", &goals), None);
    }

    #[test]
    fn a_value_spanning_lines_stays_with_its_variable() {
        let found = variables("HOST_A_X=one\n\ttwo\nHOST_B_Y=three\n");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].1, "HOST_A_X=one\n\ttwo\n");
        assert_eq!(found[1].0, "HOST_B_Y");
    }
}
