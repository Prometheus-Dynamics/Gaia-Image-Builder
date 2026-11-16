//! What one `gaia build` did, kept as `runs/<start ns>.json` next to the
//! store. `show` reads the latest; `clean` keeps what the latest N name.
//!
//! Each step carries its wall time, the CPU time of its tools, the job
//! count it was given and what its body reported as details (for a
//! Buildroot step, the time of each package). The run carries the critical
//! path: the chain of steps whose times set the total.

use crate::Stop;
use gaia_store::{Keep, ObjectRef, StepKey, Store};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub product: PathBuf,
    /// The gaia process that is building, while it runs. A record with no
    /// outcome whose process is gone is a build that died.
    pub pid: Option<u32>,
    pub start_ns: u64,
    pub end_ns: u64,
    /// `done`, `failed` or `cancelled`. None until the build ends: a record
    /// that stays None has no process left to end it.
    #[serde(default)]
    pub outcome: Option<String>,
    /// By host port, `<image>.<output>`.
    pub outputs: BTreeMap<String, Output>,
    /// Every step this run looked up or made, in the order they ended.
    pub steps: Vec<Step>,
    /// The steps that set the run's end, first to last, each with its wall
    /// time in milliseconds: the chain of dependencies that ended last.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub critical_path: Vec<PathStep>,
}

/// One step on the critical path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathStep {
    pub step: String,
    pub ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Output {
    /// The port type: `gaia:artifact` or `gaia:tree`.
    pub ty: String,
    pub object: ObjectRef,
    /// The nodes it came from, nearest last.
    pub from: Vec<Origin>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Origin {
    pub label: String,
    pub id: String,
    pub file: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub step: String,
    pub key: String,
    /// Whether the body ran; false when the store had it.
    pub made: bool,
    /// Wall time, in milliseconds.
    pub ms: u64,
    pub outputs: BTreeMap<String, ObjectRef>,
    /// When the step began and ended, in nanoseconds since the Unix epoch.
    #[serde(default)]
    pub start_ns: u64,
    #[serde(default)]
    pub end_ns: u64,
    /// CPU time of the processes its tools ran, in milliseconds.
    #[serde(default)]
    pub cpu_ms: u64,
    /// The job count the step was given: its make's `-j` when it has one,
    /// else the build's `--jobs`, its budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jobs: Option<u32>,
    /// Whether the body failed.
    #[serde(default, skip_serializing_if = "is_false")]
    pub failed: bool,
    /// What the body reported, by name (for example `packages`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, Value>,
}

/// For `skip_serializing_if`: a flag that is off is left out.
fn is_false(flag: &bool) -> bool {
    !*flag
}

/// What a record says about its build, given its outcome, its process and
/// whether that process is alive: the outcome when there is one, `running`
/// while the process lives, else `killed, no outcome`.
pub fn state_of(outcome: Option<&str>, pid: Option<u32>, alive: impl Fn(u32) -> bool) -> String {
    match (outcome, pid) {
        (Some(outcome), _) => outcome.to_string(),
        (None, Some(pid)) if alive(pid) => "running".into(),
        (None, _) => "killed, no outcome".into(),
    }
}

/// Whether `pid` is a live gaia process. A PID now used by another program
/// is not the build, so the process's name must be gaia's.
fn process_alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .is_ok_and(|name| name.trim_end().starts_with("gaia"))
}

/// The run records directory of `store`: `runs` next to its root.
pub fn runs_dir(store: &Store) -> PathBuf {
    store.root().with_file_name("runs")
}

impl Run {
    /// The record a build writes when it starts: this process, no outcome,
    /// nothing made yet. The end of the build writes it again, whole.
    pub fn started(product: PathBuf, start_ns: u64) -> Self {
        Self {
            product,
            pid: Some(std::process::id()),
            start_ns,
            end_ns: 0,
            outcome: None,
            outputs: BTreeMap::new(),
            steps: Vec::new(),
            critical_path: Vec::new(),
        }
    }

    /// What the record says about the build: its outcome, `running` while
    /// its process lives, or `killed, no outcome` when it does not.
    pub fn state(&self) -> String {
        state_of(self.outcome.as_deref(), self.pid, process_alive)
    }

    /// Writes the record as `runs/<start ns>.json`, replacing an earlier
    /// write of the same build. The file is written under another name and
    /// renamed, so a reader never sees half of it.
    pub fn write(&self, store: &Store) -> Result<PathBuf, Stop> {
        let dir = runs_dir(store);
        std::fs::create_dir_all(&dir)
            .map_err(|e| Stop::usage(format!("{}: {e}", dir.display())))?;
        let path = dir.join(format!("{}.json", self.start_ns));
        let partial = dir.join(format!("{}.json.partial", self.start_ns));
        let text = serde_json::to_string_pretty(self).map_err(Stop::usage)?;
        std::fs::write(&partial, text)
            .map_err(|e| Stop::usage(format!("{}: {e}", partial.display())))?;
        std::fs::rename(&partial, &path)
            .map_err(|e| Stop::usage(format!("{}: {e}", path.display())))?;
        Ok(path)
    }

    pub fn read(path: &Path) -> Result<Self, Stop> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Stop::usage(format!("{}: {e}", path.display())))?;
        serde_json::from_str(&text).map_err(|e| Stop::usage(format!("{}: {e}", path.display())))
    }

    /// The step entries and objects this run used.
    pub fn keep(&self) -> Keep {
        let mut keep = Keep::default();
        for step in &self.steps {
            if let Some(key) = gaia_store::Digest::parse(&step.key) {
                keep.entries.insert(StepKey(key));
            }
            keep.objects.extend(step.outputs.values().copied());
        }
        keep.objects
            .extend(self.outputs.values().map(|output| output.object));
        keep
    }
}

/// Every run record, oldest first.
pub fn all(store: &Store) -> Vec<PathBuf> {
    let mut paths: Vec<(u64, PathBuf)> = std::fs::read_dir(runs_dir(store))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let start = path
                .file_name()?
                .to_str()?
                .strip_suffix(".json")?
                .parse()
                .ok()?;
            Some((start, path))
        })
        .collect();
    paths.sort();
    paths.into_iter().map(|(_, path)| path).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timing::{self, Deps};

    fn object() -> ObjectRef {
        format!("file:sha256:{}", "ab".repeat(32))
            .parse()
            .expect("object")
    }

    fn step(name: &str) -> Step {
        Step {
            step: name.into(),
            key: "sha256:00".into(),
            made: true,
            ms: 1_500,
            outputs: BTreeMap::from([("out".to_string(), object())]),
            start_ns: 10,
            end_ns: 1_510_000_000,
            cpu_ms: 900,
            jobs: Some(13),
            failed: false,
            details: BTreeMap::new(),
        }
    }

    fn run(steps: Vec<Step>, critical_path: Vec<PathStep>) -> Run {
        Run {
            product: PathBuf::from("/p/product.toml"),
            pid: None,
            start_ns: 1,
            end_ns: 2,
            outcome: Some("done".into()),
            outputs: BTreeMap::new(),
            steps,
            critical_path,
        }
    }

    /// The run record keeps the new fields and reads back to the same run.
    #[test]
    fn a_run_record_round_trips_with_reports_and_critical_path() {
        let mut built = step("gaia.os.buildroot");
        built.failed = true;
        built.details.insert(
            "packages".into(),
            serde_json::json!([{"name": "glibc", "phases": {"build": 400.0}, "total_s": 400.0}]),
        );
        let original = run(
            vec![built, step("gaia.fs.erofs")],
            vec![PathStep {
                step: "gaia.os.buildroot".into(),
                ms: 1_500,
            }],
        );
        let text = serde_json::to_string_pretty(&original).expect("json");
        let back: Run = serde_json::from_str(&text).expect("parse");
        assert_eq!(back, original);
        let value: serde_json::Value = serde_json::from_str(&text).expect("value");
        assert_eq!(value["steps"][0]["details"]["packages"][0]["name"], "glibc");
        assert_eq!(value["steps"][0]["failed"], true);
        assert_eq!(value["steps"][0]["jobs"], 13);
        assert_eq!(value["steps"][0]["cpu_ms"], 900);
        assert_eq!(value["critical_path"][0]["ms"], 1_500);
    }

    /// Fields a step has nothing to say about are left out of the JSON, and
    /// a record written before they existed still reads.
    #[test]
    fn empty_fields_are_left_out_and_old_records_read() {
        let plain = step("gaia.fs.erofs");
        let value = serde_json::to_value(run(vec![plain], Vec::new())).expect("json");
        assert!(value.get("critical_path").is_none());
        let step_value = &value["steps"][0];
        assert!(step_value.get("details").is_none());
        assert!(step_value.get("failed").is_none());
        assert_eq!(step_value["cpu_ms"], 900);

        let old = serde_json::json!({
            "product": "p.toml",
            "start_ns": 1,
            "end_ns": 2,
            "outcome": "done",
            "outputs": {},
            "steps": [{"step": "gaia.fs.erofs", "key": "sha256:00", "made": false, "ms": 0, "outputs": {}}],
        });
        let read: Run = serde_json::from_value(old).expect("old record");
        assert_eq!(read.steps[0].cpu_ms, 0);
        assert_eq!(read.steps[0].jobs, None);
        assert!(read.critical_path.is_empty());
    }

    /// A record with no outcome whose process is gone reads as killed; one
    /// whose process lives reads as running; an outcome always wins.
    #[test]
    fn a_record_without_outcome_reads_killed_when_its_process_is_gone() {
        // A child that has been waited for is a PID that no longer runs.
        let mut child = std::process::Command::new("true").spawn().expect("spawn");
        let dead = child.id();
        child.wait().expect("wait");
        let mut record = run(Vec::new(), Vec::new());
        record.outcome = None;
        record.pid = Some(dead);
        assert_eq!(record.state(), "killed, no outcome");

        // This test process is a gaia-named process (the test binary of the
        // gaia crate), so its record reads as running.
        record.pid = Some(std::process::id());
        assert_eq!(record.state(), "running");

        record.outcome = Some("done".into());
        assert_eq!(record.state(), "done");
    }

    /// The state from the outcome, the process and whether it is alive.
    #[test]
    fn state_follows_outcome_then_process() {
        assert_eq!(state_of(None, Some(7), |_| true), "running");
        assert_eq!(state_of(None, Some(7), |_| false), "killed, no outcome");
        assert_eq!(state_of(None, None, |_| true), "killed, no outcome");
        assert_eq!(state_of(Some("failed"), Some(7), |_| true), "failed");
    }

    /// The record a build starts with: its process, no outcome, and it
    /// round-trips with the outcome null.
    #[test]
    fn a_started_record_has_its_pid_and_no_outcome() {
        let started = Run::started(PathBuf::from("/p/product.toml"), 5);
        assert_eq!(started.pid, Some(std::process::id()));
        assert_eq!(started.outcome, None);
        let text = serde_json::to_string(&started).expect("json");
        let value: serde_json::Value = serde_json::from_str(&text).expect("value");
        assert!(value["outcome"].is_null());
        assert_eq!(value["pid"], std::process::id());
        let back: Run = serde_json::from_str(&text).expect("parse");
        assert_eq!(back, started);
    }

    /// The critical path of a record's steps, through the same functions
    /// `gaia build` uses.
    #[test]
    fn a_record_s_critical_path_comes_from_its_steps() {
        let deps = Deps {
            nodes: ["gaia.os.buildroot", "gaia.fs.erofs"]
                .map(String::from)
                .into_iter()
                .collect(),
            upstream: BTreeMap::from([(
                "gaia.fs.erofs".to_string(),
                ["gaia.os.buildroot".to_string()].into_iter().collect(),
            )]),
        };
        let mut first = step("gaia.os.buildroot");
        first.start_ns = 0;
        first.end_ns = 5_000_000_000;
        let mut second = step("gaia.fs.erofs");
        second.start_ns = 5_000_000_000;
        second.end_ns = 6_000_000_000;
        let path = timing::critical_path(&[first, second], &deps);
        let names: Vec<&str> = path.iter().map(|p| p.step.as_str()).collect();
        assert_eq!(names, ["gaia.os.buildroot", "gaia.fs.erofs"]);
    }
}
