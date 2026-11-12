//! `gaia.check.units`: no service unit in the image has a start limit.
//!
//! A start limit lets systemd give up on a unit that keeps failing: after too
//! many starts in the interval, it stops restarting the unit. A unit has no
//! limit only when its effective `StartLimitIntervalSec=` is 0. The effective
//! value is the unit's own (its file and its drop-ins, the last one set wins,
//! an empty value resets it), else the manager's `DefaultStartLimitIntervalSec=`
//! (`etc/systemd/system.conf` and its drop-ins), else systemd's 10 s default.
//!
//! The units read are the `.service` files in `lib/systemd/system/`,
//! `usr/lib/systemd/system/` and `etc/systemd/system/`, the last one winning a
//! name. A link winning a name (an alias, or a mask to `/dev/null`) is not a
//! unit and is not checked. Units named in the requirements are exempt, each
//! with its reason written in the description.

use crate::common::{parse_requirements, root, unreadable, verdict};
use crate::tree;
use daedalus::prelude::*;
use gaia_step::{Ctx, StepError};
use gaia_types::Tree;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

const NAME: &str = "gaia.check.units";
/// Unit directories, lowest precedence first: a later one's unit of the same
/// name wins.
const UNIT_DIRS: [&str; 3] = [
    "lib/systemd/system",
    "usr/lib/systemd/system",
    "etc/systemd/system",
];
const MANAGER_FILE: &str = "etc/systemd/system.conf";
const MANAGER_DIR: &str = "etc/systemd/system.conf.d";
/// The keys that set a unit's interval: the current and the legacy spelling.
const INTERVAL_KEYS: [&str; 2] = ["StartLimitIntervalSec", "StartLimitInterval"];
/// The keys that set the manager's default interval.
const MANAGER_KEYS: [&str; 2] = ["DefaultStartLimitIntervalSec", "DefaultStartLimitInterval"];
/// The interval systemd uses when nothing sets one.
const SYSTEMD_DEFAULT: &str = "10s";

/// Units that may keep a start limit, by unit name.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Requirements {
    pub exempt: Vec<String>,
}

/// Fails when a service unit has a start limit. Gives the tree unchanged.
#[node(
    id = "gaia.check.units",
    inputs(store, tree, requirements),
    outputs(tree)
)]
pub fn check_units(
    store: &str,
    tree: Tree,
    requirements: &str,
    _cx: &Ctx,
) -> Result<Tree, StepError> {
    let wanted: Requirements = parse_requirements(NAME, requirements)?;
    let dir = root(NAME, store, &tree)?;
    let problems = problems(&dir, &wanted).map_err(|e| unreadable(NAME, e))?;
    verdict(NAME, problems)?;
    Ok(tree)
}

/// Every service unit with a start limit, and every limit that is not a
/// time span, as one line each.
pub fn problems(root: &Path, wanted: &Requirements) -> Result<Vec<String>, String> {
    let mut winners: BTreeMap<String, (usize, bool)> = BTreeMap::new();
    for (index, dir) in UNIT_DIRS.iter().enumerate() {
        for (name, is_link) in unit_entries(root, dir) {
            if name.ends_with(".service") {
                winners.insert(name, (index, is_link));
            }
        }
    }
    let manager = manager_default(root);
    let mut problems = Vec::new();
    for (unit, (index, is_link)) in &winners {
        if *is_link || wanted.exempt.contains(unit) {
            continue;
        }
        let mut texts = vec![read_text(root, &format!("{}/{unit}", UNIT_DIRS[*index]))];
        for dir in UNIT_DIRS {
            for file in conf_files(root, &format!("{dir}/{unit}.d")) {
                texts.push(read_text(root, &file));
            }
        }
        let mut own: Option<String> = None;
        for text in texts.iter().flatten() {
            set_keys_in(text, &["Unit", "Service"], &INTERVAL_KEYS, &mut own);
        }
        let (raw, source) = match &own {
            Some(value) => (value.clone(), "set by the unit"),
            None => match &manager {
                Some(value) => (value.clone(), "from the manager default"),
                None => (SYSTEMD_DEFAULT.to_string(), "systemd's default"),
            },
        };
        match span_seconds(&raw) {
            None => problems.push(format!(
                "unit {unit}: StartLimitIntervalSec={raw} ({source}) is not a time span"
            )),
            Some(0.0) => {}
            Some(_) => problems.push(format!(
                "unit {unit} has a start limit: StartLimitIntervalSec={raw} ({source}), it must be 0"
            )),
        }
    }
    Ok(problems)
}

/// The manager's `DefaultStartLimitIntervalSec=`, when it is set.
fn manager_default(root: &Path) -> Option<String> {
    let mut value: Option<String> = None;
    let mut texts = vec![read_text(root, MANAGER_FILE)];
    for file in conf_files(root, MANAGER_DIR) {
        texts.push(read_text(root, &file));
    }
    for text in texts.iter().flatten() {
        set_keys_in(text, &["Manager"], &MANAGER_KEYS, &mut value);
    }
    value
}

/// The entries of a unit directory: each name with whether it is a link.
fn unit_entries(root: &Path, dir: &str) -> Vec<(String, bool)> {
    let Some(found) = tree::resolve(root, dir) else {
        return Vec::new();
    };
    let Ok(items) = std::fs::read_dir(root.join(found)) else {
        return Vec::new();
    };
    items
        .filter_map(|item| {
            let item = item.ok()?;
            let kind = item.file_type().ok()?;
            let name = item.file_name().into_string().ok()?;
            (kind.is_file() || kind.is_symlink()).then(|| (name, kind.is_symlink()))
        })
        .collect()
}

/// The `.conf` files of a drop-in directory, in name order, as tree paths.
fn conf_files(root: &Path, dir: &str) -> Vec<String> {
    let mut names: Vec<String> = unit_entries(root, dir)
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| name.ends_with(".conf"))
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| format!("{dir}/{name}"))
        .collect()
}

fn read_text(root: &Path, rel: &str) -> Option<String> {
    tree::read(root, rel).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// Sets the value of any of `keys` in the given sections of one unit or
/// manager file, as the last occurrence in the file says. A non-empty value
/// sets it; an empty one resets it.
fn set_keys_in(text: &str, sections: &[&str], keys: &[&str], value: &mut Option<String>) {
    let mut section = "";
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = name;
            continue;
        }
        if !sections.contains(&section) {
            continue;
        }
        if let Some((name, setting)) = line.split_once('=')
            && keys.contains(&name.trim())
        {
            let setting = setting.trim();
            *value = (!setting.is_empty()).then(|| setting.to_string());
        }
    }
}

/// A systemd time span in seconds: `10`, `10s`, `500ms`, `1min 30s`, or
/// `infinity`. `None` when it is not one.
fn span_seconds(text: &str) -> Option<f64> {
    let text = text.trim();
    if text == "infinity" {
        return Some(f64::INFINITY);
    }
    let mut total = 0.0;
    let mut rest = text;
    let mut any = false;
    while !rest.is_empty() {
        rest = rest.trim_start();
        let end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let (number, after) = rest.split_at(end);
        let number: f64 = number.parse().ok()?;
        let unit_end = after
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(after.len());
        let (unit, after) = after.split_at(unit_end);
        let scale = match unit {
            "" | "s" | "sec" | "second" | "seconds" => 1.0,
            "us" | "usec" => 1e-6,
            "ms" | "msec" => 1e-3,
            "min" | "minute" | "minutes" => 60.0,
            "h" | "hr" | "hour" | "hours" => 3600.0,
            "d" | "day" | "days" => 86_400.0,
            _ => return None,
        };
        total += number * scale;
        rest = after;
        any = true;
    }
    any.then_some(total)
}
