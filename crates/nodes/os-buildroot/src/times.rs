//! Where a Buildroot make's time went, package by package: read from
//! Buildroot's own `build/build-time.log` and recorded in the step's record
//! as the `packages` detail.
//!
//! Buildroot appends two lines for every step of every package it builds,
//! one when the step starts and one when it ends:
//!
//! ```text
//! <epoch seconds>.<nanoseconds>:start:<step, padded>:<package, padded>
//! <epoch seconds>.<nanoseconds>:end  :<step, padded>: <package>
//! ```
//!
//! The steps are `download`, `extract`, `patch`, `configure`, `build` and
//! the `install-*` steps, which are summed as `install`. A step that starts
//! and never ends (a make that failed inside it) is the package's open
//! phase: it is named, and its time is not counted.
//!
//! The same lines are also kept as a timeline, the `package-timeline`
//! detail: every step of every package with its start and end in seconds
//! from the make's first line, so what ran beside what, and what waited,
//! can be read from the record.

use gaia_step::Work;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// The log, under a Buildroot output directory.
const LOG: &str = "build/build-time.log";
/// The detail the record keeps the packages under.
pub(crate) const DETAIL: &str = "packages";
/// The detail the record keeps the timeline under.
pub(crate) const TIMELINE: &str = "package-timeline";

/// One package's time in one make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageTime {
    pub name: String,
    /// Nanoseconds in each completed phase, by phase name.
    pub phases: BTreeMap<String, u64>,
    /// The phase that was begun and not ended when the log stopped.
    pub open: Option<String>,
}

impl PackageTime {
    /// The completed phases' time, in nanoseconds.
    pub fn total_ns(&self) -> u64 {
        self.phases.values().sum()
    }
}

/// One parsed line of the log.
struct Line {
    at_ns: u64,
    start: bool,
    step: String,
    name: String,
}

/// The phase a Buildroot step belongs to: every `install-*` step is
/// `install`, and the others keep their names.
fn phase(step: &str) -> &str {
    if step.starts_with("install") {
        "install"
    } else {
        step
    }
}

/// `<epoch seconds>.<fraction>` as nanoseconds. A fraction longer than nine
/// digits is cut; a missing one is zero.
fn epoch_ns(text: &str) -> Option<u64> {
    let (seconds, fraction) = text.split_once('.').unwrap_or((text, ""));
    let seconds: u64 = seconds.parse().ok()?;
    let mut nanos: String = fraction.chars().take(9).collect();
    while nanos.len() < 9 {
        nanos.push('0');
    }
    let nanos: u64 = nanos.parse().ok()?;
    seconds.checked_mul(1_000_000_000)?.checked_add(nanos)
}

/// A line of the log, or `None` when it is not one.
fn parse_line(line: &str) -> Option<Line> {
    let mut fields = line.splitn(4, ':').map(str::trim);
    let at_ns = epoch_ns(fields.next()?)?;
    let start = match fields.next()? {
        "start" => true,
        "end" => false,
        _ => return None,
    };
    let step = fields.next()?;
    let name = fields.next()?;
    if step.is_empty() || name.is_empty() {
        return None;
    }
    Some(Line {
        at_ns,
        start,
        step: step.to_string(),
        name: name.to_string(),
    })
}

/// The packages in `log`, most time first (then by name). A log that stops
/// in the middle of a step gives that step as the package's open phase.
pub fn package_times(log: &str) -> Vec<PackageTime> {
    let mut packages: BTreeMap<String, PackageTime> = BTreeMap::new();
    // The start of each step not yet ended, by package and step.
    let mut begun: BTreeMap<(String, String), u64> = BTreeMap::new();
    for line in log.lines().filter_map(parse_line) {
        let package = packages
            .entry(line.name.clone())
            .or_insert_with(|| PackageTime {
                name: line.name.clone(),
                phases: BTreeMap::new(),
                open: None,
            });
        let key = (line.name, line.step);
        if line.start {
            begun.insert(key, line.at_ns);
        } else if let Some(start) = begun.remove(&key) {
            let spent = line.at_ns.saturating_sub(start);
            *package.phases.entry(phase(&key.1).to_string()).or_insert(0) += spent;
        }
    }
    for ((name, step), _) in begun {
        if let Some(package) = packages.get_mut(&name) {
            package.open = Some(phase(&step).to_string());
        }
    }
    let mut all: Vec<PackageTime> = packages.into_values().collect();
    all.sort_by(|a, b| b.total_ns().cmp(&a.total_ns()).then(a.name.cmp(&b.name)));
    all
}

/// Seconds, to the millisecond.
fn seconds(ns: u64) -> f64 {
    (ns / 1_000_000) as f64 / 1_000.0
}

/// The packages as the record's detail: `[{name, phases, total_s, open}]`,
/// phases in seconds, `open` only when a phase was open.
pub fn packages_json(packages: &[PackageTime]) -> Value {
    Value::Array(
        packages
            .iter()
            .map(|package| {
                let phases: Map<String, Value> = package
                    .phases
                    .iter()
                    .map(|(name, ns)| (name.clone(), json!(seconds(*ns))))
                    .collect();
                let mut entry = json!({
                    "name": package.name,
                    "phases": phases,
                    "total_s": seconds(package.total_ns()),
                });
                if let Some(open) = &package.open {
                    entry["open"] = json!(open);
                }
                entry
            })
            .collect(),
    )
}

/// Every step of every package in `log`, in the order the steps started:
/// `{start_ns, spans: [{package, step, start_s, end_s}]}`, `start_ns` the
/// first line's time (epoch nanoseconds) and the spans' times in seconds
/// from it, to the millisecond. The step is Buildroot's own name
/// (`install-target`, not `install`). A step begun and never ended has an
/// `end_s` of null. `None` for a log with no line.
pub fn timeline(log: &str) -> Option<Value> {
    let lines: Vec<Line> = log.lines().filter_map(parse_line).collect();
    let first = lines.iter().map(|line| line.at_ns).min()?;
    let since = |at: u64| seconds(at.saturating_sub(first));
    let mut spans: Vec<(u64, String, String, Option<u64>)> = Vec::new();
    // The span each begun step is at, by package and step.
    let mut begun: BTreeMap<(String, String), usize> = BTreeMap::new();
    for line in lines {
        let key = (line.name, line.step);
        if line.start {
            begun.insert(key.clone(), spans.len());
            spans.push((line.at_ns, key.0, key.1, None));
        } else if let Some(at) = begun.remove(&key) {
            spans[at].3 = Some(line.at_ns);
        }
    }
    spans.sort_by_key(|span| span.0);
    let spans: Vec<Value> = spans
        .into_iter()
        .map(|(start, package, step, end)| {
            json!({
                "package": package,
                "step": step,
                "start_s": since(start),
                "end_s": end.map(since),
            })
        })
        .collect();
    Some(json!({ "start_ns": first, "spans": spans }))
}

/// Records the packages of the make that ran in `out` (a Buildroot output
/// directory) in `work`'s record, with the job count the make was given.
/// A missing log records nothing but the job count.
pub(crate) fn record(work: &mut Work<'_>, out: &Path, jobs: u32) {
    work.jobs(jobs);
    let Ok(log) = fs::read_to_string(out.join(LOG)) else {
        return;
    };
    let packages = package_times(&log);
    if !packages.is_empty() {
        work.detail(DETAIL, packages_json(&packages));
    }
    if let Some(timeline) = timeline(&log) {
        work.detail(TIMELINE, timeline);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real log of a build, copied from the live output of an image build:
    /// six packages, the last one (`host-gcc-final`) open at its build.
    const FIXTURE: &str = include_str!("../tests/fixtures/build-time.log");

    fn package<'a>(all: &'a [PackageTime], name: &str) -> &'a PackageTime {
        all.iter().find(|p| p.name == name).expect(name)
    }

    #[test]
    fn fixture_phases_are_timed_per_package() {
        let all = package_times(FIXTURE);
        // glibc's configure ended at 1794720634.617910561, its build started
        // at 1794720636.411567037 and ended at 1794721039.225639858.
        let glibc = package(&all, "glibc");
        assert_eq!(
            glibc.phases["build"],
            1_794_721_039_225_639_858 - 1_794_720_636_411_567_037
        );
        assert_eq!(glibc.open, None);
        assert_eq!(
            glibc.total_ns(),
            glibc.phases.values().sum::<u64>(),
            "the total is the completed phases"
        );
    }

    #[test]
    fn a_package_open_at_the_end_names_its_open_phase() {
        let all = package_times(FIXTURE);
        let gcc = package(&all, "host-gcc-final");
        assert_eq!(gcc.open.as_deref(), Some("build"));
        assert!(
            !gcc.phases.contains_key("build"),
            "an open phase is not timed"
        );
        assert!(gcc.phases.contains_key("patch"));
    }

    #[test]
    fn packages_sort_by_total_then_name() {
        let all = package_times(FIXTURE);
        let totals: Vec<u64> = all.iter().map(PackageTime::total_ns).collect();
        assert!(
            totals.windows(2).all(|pair| pair[0] >= pair[1]),
            "{totals:?}"
        );
        assert_eq!(
            all[0].name, "linux-headers",
            "its extract and build are the longest in the fixture"
        );
    }

    #[test]
    fn install_steps_are_one_phase() {
        let log = "\
100.000000000:start:install-staging: pkg
101.000000000:end  :install-staging: pkg
101.000000000:start:install-target : pkg
103.500000000:end  :install-target : pkg
";
        let all = package_times(log);
        assert_eq!(all[0].phases["install"], 3_500_000_000);
        assert_eq!(all[0].phases.len(), 1);
    }

    #[test]
    fn a_log_cut_mid_package_keeps_what_was_done() {
        let log = "\
100.000000000:start:download            : half
101.250000000:end  :download            : half
101.250000000:start:build               : half
";
        let all = package_times(log);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].phases["download"], 1_250_000_000);
        assert_eq!(all[0].open.as_deref(), Some("build"));
        let json = packages_json(&all);
        assert_eq!(json[0]["open"], "build");
        assert_eq!(json[0]["total_s"], 1.25);
    }

    #[test]
    fn junk_lines_and_ends_without_starts_are_skipped() {
        let log = "\
not a line
100.000000000:end  :patch               : orphan
100.000000000:start:patch               : ok
100.500000000:end  :patch               : ok
100.500000000:middle:patch              : ok
";
        let all = package_times(log);
        assert_eq!(all.len(), 2, "the orphan is a package with no time");
        let ok = package(&all, "ok");
        assert_eq!(ok.phases["patch"], 500_000_000);
        assert_eq!(package(&all, "orphan").total_ns(), 0);
    }

    /// The timeline keeps every step with its own name, in start order
    /// (parallel makes may write a start late), from the first line; a step
    /// never ended has no end.
    #[test]
    fn the_timeline_has_every_step_from_the_first_line() {
        let log = "\
100.000000000:start:download            : a
101.500000000:end  :download            : a
101.500000000:start:install-target      : a
101.000000000:start:configure           : b
104.000000000:end  :install-target      : a
103.250000000:end  :configure           : b
104.000000000:start:build               : b
";
        let made = timeline(log).expect("a timeline");
        assert_eq!(made["start_ns"], 100_000_000_000_u64);
        assert_eq!(
            made["spans"],
            json!([
                { "package": "a", "step": "download", "start_s": 0.0, "end_s": 1.5 },
                { "package": "b", "step": "configure", "start_s": 1.0, "end_s": 3.25 },
                { "package": "a", "step": "install-target", "start_s": 1.5, "end_s": 4.0 },
                { "package": "b", "step": "build", "start_s": 4.0, "end_s": null },
            ])
        );
        assert_eq!(timeline("no line here\n"), None);
    }

    /// The fixture's timeline agrees with its totals: glibc's build span is
    /// the time its build phase has.
    #[test]
    fn the_fixtures_timeline_agrees_with_its_totals() {
        let timeline = timeline(FIXTURE).expect("a timeline");
        let spans = timeline["spans"].as_array().expect("spans");
        let build = spans
            .iter()
            .find(|s| s["package"] == "glibc" && s["step"] == "build")
            .expect("glibc's build");
        let span =
            build["end_s"].as_f64().expect("ended") - build["start_s"].as_f64().expect("a start");
        let total = package(&package_times(FIXTURE), "glibc").phases["build"] as f64 / 1e9;
        assert!((span - total).abs() < 0.002, "{span} and {total}");
        assert!(
            spans
                .windows(2)
                .all(|pair| pair[0]["start_s"].as_f64() <= pair[1]["start_s"].as_f64()),
            "in start order"
        );
    }

    /// A make's log reaches the step's record: the job count, the packages
    /// and the timeline.
    #[test]
    fn a_makes_log_is_recorded_in_its_step() {
        use gaia_step::{Ctx, Digest, Step, StepKey, Store};
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path().join("store")).expect("store");
        let cx = Ctx::new();
        let key = StepKey(Digest::of_bytes(b"times record"));
        Step::new(&store, &cx, "gaia.test.times", key)
            .look_up_or_make(&[("out", Path::new("out"))], |work| {
                let out = work.dir().join("out");
                fs::create_dir_all(out.join("build")).expect("mkdir");
                fs::write(out.join(LOG), FIXTURE).expect("log");
                record(work, &out, 3);
                Ok(())
            })
            .expect("made");
        let records = gaia_step::step_records(&gaia_step::logs_dir(&store));
        let report = &records.last().expect("a record").report;
        assert_eq!(report.jobs, Some(3));
        assert_eq!(report.details[DETAIL][0]["name"], "linux-headers");
        let spans = report.details[TIMELINE]["spans"].as_array().expect("spans");
        assert_eq!(
            spans.len(),
            FIXTURE.lines().filter(|l| l.contains(":start:")).count(),
            "a span for every start"
        );
    }

    #[test]
    fn json_has_name_phases_and_total_in_seconds() {
        let all = package_times(FIXTURE);
        let json = packages_json(&all);
        let first = &json[0];
        assert_eq!(first["name"], all[0].name.as_str());
        assert!(first["phases"]["build"].is_f64());
        assert!(
            (first["total_s"].as_f64().expect("number") - all[0].total_ns() as f64 / 1e9).abs()
                < 0.01
        );
        assert!(
            json.as_array()
                .expect("array")
                .iter()
                .all(|p| p.get("open").is_none() || p["open"] == "build")
        );
    }
}
