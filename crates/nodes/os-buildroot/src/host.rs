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
//! build is keyed on the Buildroot source, that list, a digest of those
//! values, and the input trees the values name. So what changes how a host
//! tool is built changes the key, and nothing else does.

use crate::{BuildrootHost, OsConfig, Prepared, configure, make_tool, prepare};
use daedalus::prelude::*;
use daedalus_cpu_author::FanIn;
use gaia_step::{Ctx, Digest, KeyBuilder, Step, StepError, Store, Stream, Tool, Work};
use gaia_types::Tree;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const PLAN: &str = "gaia.os.buildroot.host-plan";
const NAME: &str = "gaia.os.buildroot.host";
const KEY_VERSION: u32 = 1;
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
    let text = fs::read_to_string(store.path(&planned.outputs["plan"]))
        .map_err(|e| StepError::body(PLAN, format!("the plan: {e}")))?;
    let goals = text.lines().next().unwrap_or_default().to_string();
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
