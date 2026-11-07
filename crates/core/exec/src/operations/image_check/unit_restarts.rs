//! How the image's services recover from a crash (warnings only).
//!
//! A service that crashes should be restarted, never given up on, with a
//! backoff of at most 10 s. Switching a misbehaving one off is a deliberate,
//! written-down setting, not systemd's default. That default is a start
//! limit of 5 starts in 10 s: once a crash-looping unit reaches it, systemd
//! stops restarting it and the unit stays off until someone intervenes.
//!
//! For each service unit in `target/`, its drop-ins (`<unit>.d/*.conf`)
//! applied, the check warns about:
//!
//! - `unit_start_limit`: a unit with `Restart=` set keeps a start limit,
//!   because neither it nor the manager's `DefaultStartLimitIntervalSec`
//!   sets the interval to 0;
//! - `unit_restart_slow`: `RestartSec=` over 10 s;
//! - `unit_no_restart`: an enabled long-running service (`Type=` simple,
//!   exec, notify or notify-reload) with no `Restart=`, or `Restart=no`.
//!
//! Units the `systemd` package installs are systemd's own and are left out.
//! `[[image.checks.unit_restarts]]` allows a unit with a reason.
use super::kernel_modules::Outcome;
use gaia_spec::ResolvedBuildSpec;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// The longest restart delay the policy allows.
const MAX_RESTART_SEC: f64 = 10.0;

/// Unit directories, lowest precedence first: a later one's file of the same
/// name replaces an earlier one's.
const UNIT_DIRS: &[&str] = &[
    "lib/systemd/system",
    "usr/lib/systemd/system",
    "etc/systemd/system",
];

pub(super) fn check(spec: &ResolvedBuildSpec, rootfs: &Path, outcome: &mut Outcome) {
    let allowed: BTreeSet<&str> = spec
        .image
        .checks
        .unit_restarts
        .iter()
        .map(|allow| allow.unit.as_str())
        .collect();
    let systemd_owned = systemd_owned_units(rootfs);
    for finding in findings(rootfs, &systemd_owned) {
        if allowed.contains(finding.unit.as_str()) {
            continue;
        }
        outcome.warning(finding.code, Some(&finding.unit), finding.message);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct UnitFinding {
    pub code: &'static str,
    pub unit: String,
    pub message: String,
}

/// The findings for the services under `rootfs`, by unit name.
pub(super) fn findings(rootfs: &Path, skip: &BTreeSet<String>) -> Vec<UnitFinding> {
    let units = unit_files(rootfs);
    let enabled = enabled_units(rootfs);
    let manager_no_limit = manager_start_limit_off(rootfs);
    let mut out = Vec::new();
    for (name, rel) in &units {
        if !name.ends_with(".service") || skip.contains(name) {
            continue;
        }
        let mut text = fs::read_to_string(rootfs.join(rel)).unwrap_or_default();
        for drop_in in drop_ins(rootfs, name) {
            text.push('\n');
            text.push_str(&drop_in);
        }
        let settings = Settings::parse(&text);
        let restart = settings.last("Restart").unwrap_or("no");
        let restarts = restart != "no";
        if restarts {
            let interval = settings
                .last("StartLimitIntervalSec")
                .or_else(|| settings.last("StartLimitInterval"));
            let limit_off = match interval {
                Some(value) => parse_seconds(value) == Some(0.0),
                None => manager_no_limit,
            };
            if !limit_off {
                out.push(UnitFinding {
                    code: "unit_start_limit",
                    unit: name.clone(),
                    message: format!(
                        "'{name}' ({rel}) has Restart={restart} but keeps systemd's start limit (5 starts in 10 s by default): after a crash loop it stays off; set StartLimitIntervalSec=0 in [Unit], or allow it with [[image.checks.unit_restarts]] unit = \"{name}\" and a reason"
                    ),
                });
            }
            if let Some(delay) = settings.last("RestartSec")
                && parse_seconds(delay).is_some_and(|seconds| seconds > MAX_RESTART_SEC)
            {
                out.push(UnitFinding {
                    code: "unit_restart_slow",
                    unit: name.clone(),
                    message: format!(
                        "'{name}' ({rel}) waits RestartSec={delay} before a restart; keep it at 10 s or less"
                    ),
                });
            }
        } else if enabled.contains(name.as_str()) {
            let kind = settings.last("Type").unwrap_or("simple");
            if matches!(kind, "simple" | "exec" | "notify" | "notify-reload") {
                out.push(UnitFinding {
                    code: "unit_no_restart",
                    unit: name.clone(),
                    message: format!(
                        "'{name}' ({rel}) is an enabled Type={kind} service without Restart=: a crash leaves it off; set Restart=always (or on-failure) with RestartSec of 10 s or less"
                    ),
                });
            }
        }
    }
    out
}

/// `[Section]` keys of a unit and its drop-ins, in order; an empty value
/// resets a key, as systemd does for the settings read here.
struct Settings {
    values: BTreeMap<String, String>,
}

impl Settings {
    fn parse(text: &str) -> Self {
        let mut values = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.starts_with(';') || line.starts_with('[') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            if value.is_empty() {
                values.remove(key);
            } else {
                values.insert(key.to_string(), value.to_string());
            }
        }
        Self { values }
    }

    fn last(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }
}

/// A systemd time span in seconds: `10`, `10s`, `500ms`, `1min 30s`,
/// `infinity` (none).
pub(super) fn parse_seconds(value: &str) -> Option<f64> {
    if value == "infinity" {
        return None;
    }
    let mut total = 0.0;
    let mut rest = value.trim();
    while !rest.is_empty() {
        let digits = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let number: f64 = rest[..digits].parse().ok()?;
        rest = rest[digits..].trim_start();
        let unit_len = rest
            .find(|c: char| c.is_ascii_digit() || c.is_whitespace())
            .unwrap_or(rest.len());
        let scale = match &rest[..unit_len] {
            "" | "s" | "sec" | "second" | "seconds" => 1.0,
            "ms" | "msec" => 0.001,
            "us" | "usec" => 0.000_001,
            "m" | "min" | "minute" | "minutes" => 60.0,
            "h" | "hr" | "hour" | "hours" => 3600.0,
            _ => return None,
        };
        total += number * scale;
        rest = rest[unit_len..].trim_start();
    }
    Some(total)
}

/// Unit name to its file (relative to the rootfs), the highest-precedence
/// directory winning. Symbolic links that alias a unit are left out.
fn unit_files(rootfs: &Path) -> BTreeMap<String, String> {
    let mut units = BTreeMap::new();
    let mut seen_dirs = BTreeSet::new();
    for dir in UNIT_DIRS {
        let Ok(canonical) = fs::canonicalize(rootfs.join(dir)) else {
            continue;
        };
        if !seen_dirs.insert(canonical) {
            continue;
        }
        let Ok(listing) = fs::read_dir(rootfs.join(dir)) else {
            continue;
        };
        for entry in listing.flatten() {
            let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_file {
                units.insert(name.clone(), format!("{dir}/{name}"));
            } else if entry.file_type().is_ok_and(|kind| kind.is_symlink()) {
                // A masked unit (a link to /dev/null) is off on purpose.
                if fs::read_link(entry.path()).is_ok_and(|target| target == Path::new("/dev/null"))
                {
                    units.remove(&name);
                }
            }
        }
    }
    units
}

/// The drop-in texts for `name` (and its template, for an instance), in
/// directory order then file name order.
fn drop_ins(rootfs: &Path, name: &str) -> Vec<String> {
    let mut texts = Vec::new();
    for dir in UNIT_DIRS {
        let Ok(listing) = fs::read_dir(rootfs.join(dir).join(format!("{name}.d"))) else {
            continue;
        };
        let mut files: Vec<_> = listing
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "conf"))
            .collect();
        files.sort();
        texts.extend(
            files
                .iter()
                .filter_map(|path| fs::read_to_string(path).ok()),
        );
    }
    texts
}

/// Units linked from a `*.wants` or `*.requires` directory: enabled. An
/// instance (`getty@tty1.service`) counts as its template.
fn enabled_units(rootfs: &Path) -> BTreeSet<String> {
    let mut enabled = BTreeSet::new();
    for dir in UNIT_DIRS {
        let Ok(listing) = fs::read_dir(rootfs.join(dir)) else {
            continue;
        };
        for entry in listing.flatten() {
            let wants = entry.file_name().to_string_lossy().into_owned();
            if !(wants.ends_with(".wants") || wants.ends_with(".requires")) {
                continue;
            }
            for link in fs::read_dir(entry.path()).into_iter().flatten().flatten() {
                let name = link.file_name().to_string_lossy().into_owned();
                enabled.insert(template_of(&name));
            }
        }
    }
    enabled
}

fn template_of(name: &str) -> String {
    match (name.split_once('@'), name.rsplit_once('.')) {
        (Some((prefix, _)), Some((_, suffix))) => format!("{prefix}@.{suffix}"),
        _ => name.to_string(),
    }
}

/// Whether the manager's `DefaultStartLimitIntervalSec` is 0
/// (`etc/systemd/system.conf` and its drop-ins).
fn manager_start_limit_off(rootfs: &Path) -> bool {
    let mut text = fs::read_to_string(rootfs.join("etc/systemd/system.conf")).unwrap_or_default();
    for dir in ["usr/lib/systemd/system.conf.d", "etc/systemd/system.conf.d"] {
        let mut files: Vec<_> = fs::read_dir(rootfs.join(dir))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        files.sort();
        for file in files {
            text.push('\n');
            text.push_str(&fs::read_to_string(file).unwrap_or_default());
        }
    }
    Settings::parse(&text)
        .last("DefaultStartLimitIntervalSec")
        .and_then(parse_seconds)
        == Some(0.0)
}

/// The service units the `systemd` package installs, from Buildroot's
/// `build/packages-file-list.txt` next to `target/` (empty without one).
fn systemd_owned_units(rootfs: &Path) -> BTreeSet<String> {
    let Some(output) = rootfs.parent() else {
        return BTreeSet::new();
    };
    let list = fs::read_to_string(output.join("build/packages-file-list.txt")).unwrap_or_default();
    list.lines()
        .filter_map(|line| line.split_once(','))
        .filter(|(pkg, path)| *pkg == "systemd" && path.contains("/systemd/system/"))
        .filter_map(|(_, path)| path.rsplit('/').next())
        .filter(|name| name.ends_with(".service"))
        .map(str::to_string)
        .collect()
}
