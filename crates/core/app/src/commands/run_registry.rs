//! The per-user registry of running `gaia run`s. `gaia status`, `pause`,
//! `resume`, `cancel` and `tui` use it to find every run on the system from
//! any directory, without resolving the build config of each run.
//!
//! A run writes `<runs dir>/<pid>.json` when it starts (through a temp file
//! and a rename) and removes it when it ends. Ending also writes
//! `<runs dir>/ended/<pid>.json`, which points at the run's final snapshot
//! and is kept for [`ENDED_RETENTION_SECS`]. Reading the registry drops the
//! entries whose pid is gone or now belongs to another program, so a crashed
//! run does not linger. A paused run keeps its entry: it is still a gaia
//! process.
//!
//! The runs dir is `$GAIA_RUNS_DIR` when set, else `$XDG_RUNTIME_DIR/gaia/runs`,
//! else `${XDG_STATE_HOME:-$HOME/.local/state}/gaia/runs`.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use gaia_spec::ResolvedBuildSpec;
use serde::{Deserialize, Serialize};

use super::live_status::{
    LastRun, LiveRun, RunFiles, live_run_for_pid, read_json, read_last_at, unix_now,
    write_json_atomic,
};

/// Set by the `gaia` binary (see [`enable_recording`]).
static RECORD_RUNS: AtomicBool = AtomicBool::new(false);

/// Overrides the runs dir (tests and unusual setups).
pub(crate) const RUNS_DIR_ENV: &str = "GAIA_RUNS_DIR";
const ENDED_DIR: &str = "ended";
/// How long a run that ended stays listed.
pub(crate) const ENDED_RETENTION_SECS: u64 = 24 * 60 * 60;

/// What a `gaia run` registers about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RegisteredRun {
    pub pid: u32,
    pub build_id: String,
    pub build_name: String,
    pub display_name: String,
    /// Absolute path of the build config the run was started with.
    pub build_config: PathBuf,
    /// Working directory of the run when it started.
    pub cwd: PathBuf,
    /// The full command line of the run.
    pub argv: Vec<String>,
    pub build_dir: PathBuf,
    pub pid_file: PathBuf,
    pub status_file: PathBuf,
    pub last_file: PathBuf,
    /// Unix seconds when the run started.
    pub started_at: u64,
}

/// A run that ended: `ended/<pid>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EndedRecord {
    #[serde(flatten)]
    pub run: RegisteredRun,
    /// Unix seconds when the run ended.
    pub ended_at: u64,
}

/// A registered run with what its files say now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListedRun {
    pub run: RegisteredRun,
    /// Set while the run is live.
    pub live: Option<LiveRun>,
    /// Set once the run has ended: its final snapshot and outcome.
    pub ended: Option<LastRun>,
}

impl ListedRun {
    pub(crate) fn is_live(&self) -> bool {
        self.live.is_some()
    }

    /// Unix seconds when the run ended, 0 while it is live or unknown.
    pub(crate) fn ended_at(&self) -> u64 {
        self.ended.as_ref().map_or(0, |last| last.ended_at)
    }
}

impl RegisteredRun {
    /// The registration of the `gaia run` of `spec`, started with
    /// `build_config` as its build file.
    pub(crate) fn for_run(spec: &ResolvedBuildSpec, build_config: &str) -> Self {
        let build_dir = absolute(Path::new(&spec.workspace.build_dir));
        let files = RunFiles::in_build_dir(&build_dir);
        Self {
            pid: std::process::id(),
            build_id: spec.identity.id.as_str().to_string(),
            build_name: spec.build_name().to_string(),
            display_name: spec.display_name().to_string(),
            build_config: absolute(Path::new(build_config)),
            cwd: env::current_dir().unwrap_or_default(),
            argv: env::args().collect(),
            build_dir,
            pid_file: files.pid,
            status_file: files.status,
            last_file: files.last,
            started_at: unix_now(),
        }
    }

    #[cfg_attr(not(feature = "tui"), allow(dead_code))]
    pub(crate) fn files(&self) -> RunFiles {
        RunFiles {
            pid: self.pid_file.clone(),
            status: self.status_file.clone(),
            last: self.last_file.clone(),
        }
    }

    /// Whether `selector` names this run: its display name (any case), build
    /// name, build id, or its build config path.
    pub(crate) fn matches(&self, selector: &str) -> bool {
        self.display_name.eq_ignore_ascii_case(selector)
            || self.build_name == selector
            || self.build_id == selector
            || self.is_config(selector)
    }

    /// Whether `path` is this run's build config.
    pub(crate) fn is_config(&self, path: &str) -> bool {
        absolute(Path::new(path)) == self.build_config
    }
}

/// Holds a run's registry entry. The entry is removed when the guard drops,
/// so a run that stops early (an error, a panic) leaves nothing behind.
pub(crate) struct RunRegistration {
    dir: PathBuf,
    run: RegisteredRun,
}

impl RunRegistration {
    /// Registers a run that is starting in the default runs dir. Errors are
    /// ignored: the registry is best effort and must not stop a build.
    pub(crate) fn begin(run: RegisteredRun) -> Self {
        Self::begin_in(runs_dir(), run)
    }

    pub(crate) fn begin_in(dir: PathBuf, run: RegisteredRun) -> Self {
        let _ = fs::create_dir_all(&dir)
            .and_then(|()| write_json_atomic(&entry_path(&dir, run.pid), &run));
        Self { dir, run }
    }

    /// The run ended at `ended_at`: records it under `ended/`. The live entry
    /// is removed when the guard drops, just after.
    pub(crate) fn end(self, ended_at: u64) {
        write_ended(&self.dir, &self.run, ended_at);
    }
}

impl Drop for RunRegistration {
    fn drop(&mut self) {
        let _ = fs::remove_file(entry_path(&self.dir, self.run.pid));
    }
}

/// Makes this process register the runs it executes. Only the `gaia` binary
/// does: library callers, such as the tests, leave the user's registry alone.
pub(crate) fn enable_recording() {
    RECORD_RUNS.store(true, Ordering::Relaxed);
}

pub(crate) fn recording_enabled() -> bool {
    RECORD_RUNS.load(Ordering::Relaxed)
}

/// The runs dir of this user (see the module docs).
pub(crate) fn runs_dir() -> PathBuf {
    if let Some(dir) = non_empty_env(RUNS_DIR_ENV) {
        return PathBuf::from(dir);
    }
    if let Some(runtime) = non_empty_env("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime).join("gaia").join("runs");
    }
    let state = non_empty_env("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            non_empty_env("HOME").map(|home| PathBuf::from(home).join(".local").join("state"))
        })
        .unwrap_or_else(env::temp_dir);
    state.join("gaia").join("runs")
}

fn non_empty_env(name: &str) -> Option<OsString> {
    env::var_os(name).filter(|value| !value.is_empty())
}

/// Every registered run in the default runs dir, live runs first (oldest
/// first), then the runs that ended in the retention window (newest first).
pub(crate) fn list_runs() -> Vec<ListedRun> {
    list_runs_in(&runs_dir(), unix_now())
}

/// [`list_runs`] for `dir`, judged at `now`.
pub(crate) fn list_runs_in(dir: &Path, now: u64) -> Vec<ListedRun> {
    if !cfg!(unix) {
        return Vec::new();
    }
    let mut live = Vec::new();
    for (_, run) in read_json_dir::<RegisteredRun>(dir) {
        let listed = inspect_run(run);
        if listed.is_live() {
            live.push(listed);
            continue;
        }
        let _ = fs::remove_file(entry_path(dir, listed.run.pid));
        // A run that died after writing its final snapshot still ended.
        if let Some(last) = listed.ended {
            write_ended(dir, &listed.run, last.ended_at);
        }
    }
    let mut ended = Vec::new();
    for (path, record) in read_json_dir::<EndedRecord>(&dir.join(ENDED_DIR)) {
        let expired = now.saturating_sub(record.ended_at) > ENDED_RETENTION_SECS;
        // The run's last-run file is per build dir: a later run there replaces
        // it, and then this run's outcome is gone.
        let last =
            read_last_at(&record.run.last_file).filter(|last| last.status.pid == record.run.pid);
        match last {
            Some(last) if !expired => ended.push(ListedRun {
                run: record.run,
                live: None,
                ended: Some(last),
            }),
            _ => {
                let _ = fs::remove_file(path);
            }
        }
    }

    live.sort_by_key(|run| (run.run.started_at, run.run.pid));
    ended.sort_by_key(|run| std::cmp::Reverse(run.ended_at()));
    live.extend(ended);
    live
}

/// The state of one registered run now: live (its pid is a gaia process),
/// or ended with its final snapshot.
pub(crate) fn inspect_run(run: RegisteredRun) -> ListedRun {
    if pid_is_gaia(run.pid) {
        let live = Some(live_run_for_pid(run.pid, &run.status_file));
        return ListedRun {
            run,
            live,
            ended: None,
        };
    }
    let ended = read_last_at(&run.last_file).filter(|last| last.status.pid == run.pid);
    ListedRun {
        run,
        live: None,
        ended,
    }
}

/// The run `selector` names, by position in `runs` (1-based, as `gaia status`
/// numbers them), by display name, build name, build id or build config
/// path. A name that matches a live run and an ended one selects the live
/// run. `Ok(None)` when nothing registered matches.
pub(crate) fn select_run<'a>(
    runs: &'a [ListedRun],
    selector: &str,
) -> Result<Option<&'a ListedRun>, String> {
    if let Ok(number) = selector.parse::<usize>() {
        return match number.checked_sub(1).and_then(|index| runs.get(index)) {
            Some(run) => Ok(Some(run)),
            None => Err(format!(
                "no run number {number}; {} run(s) listed by gaia status",
                runs.len()
            )),
        };
    }
    let mut matches: Vec<(usize, &ListedRun)> = runs
        .iter()
        .enumerate()
        .filter(|(_, run)| run.run.matches(selector))
        .collect();
    if matches.iter().any(|(_, run)| run.is_live()) {
        matches.retain(|(_, run)| run.is_live());
    }
    match matches.as_slice() {
        [] => Ok(None),
        [(_, run)] => Ok(Some(*run)),
        many => Err(format!(
            "'{selector}' matches {} runs; give its number instead: {}",
            many.len(),
            many.iter()
                .map(|(index, run)| format!("{} (pid {})", index + 1, run.run.pid))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn write_ended(dir: &Path, run: &RegisteredRun, ended_at: u64) {
    let record = EndedRecord {
        run: run.clone(),
        ended_at,
    };
    let ended_dir = dir.join(ENDED_DIR);
    let _ = fs::create_dir_all(&ended_dir)
        .and_then(|()| write_json_atomic(&ended_dir.join(file_name(run.pid)), &record));
}

/// The `.json` files directly in `dir`, decoded. Files that do not decode are
/// removed: they can only be left over from a crash.
fn read_json_dir<T: for<'de> Deserialize<'de>>(dir: &Path) -> Vec<(PathBuf, T)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        match read_json::<T>(&path) {
            Some(value) => out.push((path, value)),
            None => {
                let _ = fs::remove_file(&path);
            }
        }
    }
    out
}

fn file_name(pid: u32) -> String {
    format!("{pid}.json")
}

fn entry_path(dir: &Path, pid: u32) -> PathBuf {
    dir.join(file_name(pid))
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(unix)]
fn pid_is_gaia(pid: u32) -> bool {
    libc::pid_t::try_from(pid)
        .ok()
        .and_then(super::interrupt::gaia_process_alive)
        .is_some()
}

#[cfg(not(unix))]
fn pid_is_gaia(_pid: u32) -> bool {
    false
}

#[cfg(test)]
pub(crate) fn fixture_registered_run(pid: u32, display_name: &str) -> RegisteredRun {
    let build_dir = PathBuf::from("/builds/demo/build");
    let files = RunFiles::in_build_dir(&build_dir);
    RegisteredRun {
        pid,
        build_id: "demo".into(),
        build_name: "demo".into(),
        display_name: display_name.into(),
        build_config: PathBuf::from("/builds/demo.toml"),
        cwd: PathBuf::from("/builds"),
        argv: vec!["gaia".into(), "run".into(), "demo.toml".into()],
        build_dir,
        pid_file: files.pid,
        status_file: files.status,
        last_file: files.last,
        started_at: 1_000,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::commands::live_status::{FinishedCounts, LiveStatus};
    use std::process::Command;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("gaia-run-registry-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// A pid of a process that has exited and been reaped: certainly dead.
    fn dead_pid() -> u32 {
        let mut child = Command::new("true").spawn().expect("spawn true");
        let pid = child.id();
        child.wait().expect("wait true");
        pid
    }

    /// A live process that is not gaia: a `sleep` child.
    fn foreign_pid() -> (u32, std::process::Child) {
        let child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        (child.id(), child)
    }

    /// The test binary is named `gaia_app-*`, so its own pid counts as a
    /// live gaia process.
    fn own_pid() -> u32 {
        std::process::id()
    }

    fn final_snapshot(pid: u32, ended_at: u64) -> LastRun {
        LastRun {
            outcome: "completed".into(),
            ended_at,
            status: LiveStatus {
                pid,
                build_name: "demo".into(),
                display_name: "Demo build".into(),
                started_at: 1_000,
                updated_at: ended_at,
                paused: false,
                ops_total: 4,
                ops_done: 4,
                running: Vec::new(),
                finished: FinishedCounts {
                    done: 4,
                    ..FinishedCounts::default()
                },
                failed: Vec::new(),
                logs: Vec::new(),
            },
            errors: Vec::new(),
        }
    }

    fn write_last(run: &RegisteredRun, last: &LastRun) {
        fs::create_dir_all(run.last_file.parent().expect("parent")).expect("build dir");
        write_json_atomic(&run.last_file, last).expect("last file");
    }

    #[test]
    fn registration_is_written_and_removed_on_drop() {
        let dir = temp_dir("write-remove");
        let run = fixture_registered_run(own_pid(), "Demo build");
        let entry = dir.join(format!("{}.json", run.pid));
        {
            let _registration = RunRegistration::begin_in(dir.clone(), run.clone());
            let written: RegisteredRun = read_json(&entry).expect("entry written");
            assert_eq!(written, run);
            assert!(!dir.join(format!("{}.json.tmp", run.pid)).exists());
        }
        assert!(!entry.exists(), "dropping the guard removes the entry");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ended_run_is_listed_for_a_day_and_its_entry_is_gone() {
        let dir = temp_dir("ended");
        let build = temp_dir("ended-build");
        let mut run = fixture_registered_run(dead_pid(), "Ended build");
        run.build_dir = build.clone();
        let files = RunFiles::in_build_dir(&build);
        run.pid_file = files.pid;
        run.status_file = files.status;
        run.last_file = files.last;
        let registration = RunRegistration::begin_in(dir.clone(), run.clone());
        // The run finished: its final snapshot is written, then the guard ends.
        let last = final_snapshot(run.pid, 2_000);
        write_last(&run, &last);
        registration.end(2_000);

        let listed = list_runs_in(&dir, 2_100);
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert!(!listed[0].is_live());
        assert_eq!(listed[0].ended.as_ref(), Some(&last));
        assert!(!entry_path(&dir, run.pid).exists());
        assert!(ended_path_for(&dir, run.pid).exists());

        // After 24 hours the record is pruned.
        let later = list_runs_in(&dir, 2_000 + ENDED_RETENTION_SECS + 1);
        assert!(later.is_empty());
        assert!(!ended_path_for(&dir, run.pid).exists());
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(build);
    }

    fn ended_path_for(dir: &Path, pid: u32) -> PathBuf {
        dir.join(ENDED_DIR).join(file_name(pid))
    }

    #[test]
    fn stale_entries_of_dead_or_foreign_processes_are_pruned() {
        let dir = temp_dir("stale");
        let dead = fixture_registered_run(dead_pid(), "Dead build");
        let (foreign, mut sleeper) = foreign_pid();
        let foreign = fixture_registered_run(foreign, "Foreign build");
        // Written by hand: the registration guard would remove them again.
        write_json_atomic(&entry_path(&dir, dead.pid), &dead).expect("dead entry");
        write_json_atomic(&entry_path(&dir, foreign.pid), &foreign).expect("foreign entry");
        fs::write(dir.join("junk.json"), b"{not json").expect("junk");

        let listed = list_runs_in(&dir, 1_000);
        let _ = sleeper.kill();
        let _ = sleeper.wait();
        assert!(listed.is_empty(), "{listed:?}");
        assert!(!entry_path(&dir, dead.pid).exists());
        assert!(!entry_path(&dir, foreign.pid).exists());
        assert!(!dir.join("junk.json").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_run_that_died_after_its_final_snapshot_is_listed_as_ended() {
        let dir = temp_dir("crashed");
        let build = temp_dir("crashed-build");
        let files = RunFiles::in_build_dir(&build);
        let mut run = fixture_registered_run(dead_pid(), "Crashed build");
        run.pid_file = files.pid;
        run.status_file = files.status;
        run.last_file = files.last;
        write_json_atomic(&entry_path(&dir, run.pid), &run).expect("entry");
        write_last(&run, &final_snapshot(run.pid, 1_500));

        let listed = list_runs_in(&dir, 1_600);
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert_eq!(listed[0].ended_at(), 1_500);
        assert!(!entry_path(&dir, run.pid).exists());
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(build);
    }

    #[test]
    fn a_live_run_is_listed_with_its_status_and_a_paused_run_keeps_its_entry() {
        let dir = temp_dir("live");
        let build = temp_dir("live-build");
        let files = RunFiles::in_build_dir(&build);
        let mut run = fixture_registered_run(own_pid(), "Live build");
        run.status_file = files.status.clone();
        run.last_file = files.last;
        run.pid_file = files.pid;
        let mut status = crate::commands::live_status::fixture_status();
        status.pid = own_pid();
        write_json_atomic(&files.status, &status).expect("status");
        let registration = RunRegistration::begin_in(dir.clone(), run.clone());

        let listed = list_runs_in(&dir, 1_100);
        assert_eq!(listed.len(), 1, "{listed:?}");
        let live = listed[0].live.as_ref().expect("live");
        assert_eq!(live.status.as_ref(), Some(&status));
        assert!(entry_path(&dir, run.pid).exists());
        drop(registration);
        assert!(!entry_path(&dir, run.pid).exists());
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(build);
    }

    #[test]
    fn selection_by_number_name_and_config_path() {
        let build_a = fixture_registered_run(own_pid(), "Alpha");
        let mut build_b = fixture_registered_run(own_pid(), "Beta");
        build_b.build_name = "beta".into();
        build_b.build_id = "beta-id".into();
        build_b.build_config = PathBuf::from("/builds/beta.toml");
        let live_a = ListedRun {
            run: build_a.clone(),
            live: Some(LiveRun {
                pid: own_pid(),
                status: None,
                stopped: false,
            }),
            ended: None,
        };
        let ended_b = ListedRun {
            run: build_b.clone(),
            live: None,
            ended: Some(final_snapshot(build_b.pid, 2_000)),
        };
        let runs = vec![live_a, ended_b];

        assert_eq!(
            select_run(&runs, "1").unwrap().map(|r| &r.run),
            Some(&build_a)
        );
        assert_eq!(
            select_run(&runs, "2").unwrap().map(|r| &r.run),
            Some(&build_b)
        );
        assert!(select_run(&runs, "3").is_err());
        assert!(select_run(&runs, "0").is_err());
        assert_eq!(
            select_run(&runs, "alpha").unwrap().map(|r| &r.run),
            Some(&build_a)
        );
        assert_eq!(
            select_run(&runs, "beta-id").unwrap().map(|r| &r.run),
            Some(&build_b)
        );
        assert_eq!(
            select_run(&runs, "/builds/beta.toml")
                .unwrap()
                .map(|r| &r.run),
            Some(&build_b)
        );
        assert_eq!(select_run(&runs, "gamma").unwrap(), None);
    }

    #[test]
    fn a_name_shared_by_two_runs_asks_for_a_number() {
        let first = fixture_registered_run(own_pid(), "Same");
        let second = fixture_registered_run(own_pid().wrapping_add(1), "Same");
        let runs = vec![
            ListedRun {
                run: first,
                live: Some(LiveRun {
                    pid: own_pid(),
                    status: None,
                    stopped: false,
                }),
                ended: None,
            },
            ListedRun {
                run: second,
                live: None,
                ended: Some(final_snapshot(own_pid(), 2_000)),
            },
        ];
        // The live one wins over the ended one.
        assert_eq!(
            select_run(&runs, "same").unwrap().map(|r| r.is_live()),
            Some(true)
        );
    }
}
