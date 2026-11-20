//! Look up or make.

use crate::error::ErrorKind;
use crate::log::{Report, RunLog};
use crate::{Ctx, StepError, Tool, ToolOutput, tool};
use gaia_store::{Check, ObjectRef, StepKey, Store};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What Daedalus keeps of a fault's reason.
const REASON_BYTES: usize = 120;

/// One run of one step: where its outputs go, how it is cancelled, and its
/// key.
pub struct Step<'a> {
    store: &'a Store,
    cx: &'a Ctx,
    name: &'a str,
    key: StepKey,
    check: Check,
    logs: PathBuf,
    ram_work: Option<u64>,
    ram_floor: u64,
}

/// What [`Step::look_up_or_make`] gives back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Made {
    /// The step's outputs, by name.
    pub outputs: BTreeMap<String, ObjectRef>,
    /// Whether the body ran. `false` means every output was already in the
    /// store under this key.
    pub made: bool,
}

impl std::fmt::Debug for Step<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Step")
            .field("name", &self.name)
            .field("key", &self.key)
            .field("check", &self.check)
            .finish_non_exhaustive()
    }
}

impl<'a> Step<'a> {
    /// `name` names the step in faults; use the node id.
    pub fn new(store: &'a Store, cx: &'a Ctx, name: &'a str, key: StepKey) -> Self {
        Self {
            store,
            cx,
            name,
            key,
            check: Check::Digest,
            logs: crate::logs_dir(store),
            ram_work: None,
            ram_floor: 0,
        }
    }

    /// Lets the body's work directory be in RAM when the environment allows
    /// it (`GAIA_WORK_DIR`, `GAIA_RAM_MAX`; see `gaia_store::Store::scratch_for`).
    /// `default_bytes` is the size expected when no earlier run of this step
    /// recorded its own: the work directory's size is recorded for the next
    /// run. The outputs are committed as usual, copied from RAM when needed.
    pub fn ram_work(mut self, default_bytes: u64) -> Self {
        self.ram_work = Some(default_bytes);
        self
    }

    /// The least work directory size to expect for this run, whatever the
    /// records say: a size the step knows from its inputs before it runs,
    /// which an earlier run of other inputs cannot have measured. Only read
    /// with [`Step::ram_work`].
    pub fn ram_floor(mut self, bytes: u64) -> Self {
        self.ram_floor = bytes;
        self
    }

    /// Where this step's run logs go, when not [`crate::logs_dir`].
    pub fn logs(mut self, dir: impl Into<PathBuf>) -> Self {
        self.logs = dir.into();
        self
    }

    /// How an entry's outputs are checked before they are used. The default,
    /// [`Check::Digest`], reads each output again and compares its digest;
    /// [`Check::Present`] only checks that it is there.
    pub fn check(mut self, check: Check) -> Self {
        self.check = check;
        self
    }

    /// Returns the outputs recorded for this step's key when they are all in
    /// the store. Otherwise runs `body` in a fresh output directory, commits
    /// `outputs` (names, and paths relative to that directory) and records
    /// them under the key. A body that fails leaves nothing behind.
    pub fn look_up_or_make<F>(self, outputs: &[(&str, &Path)], body: F) -> Result<Made, StepError>
    where
        F: FnOnce(&mut Work<'_>) -> Result<(), StepError>,
    {
        let store_error = StepError::store(self.name);
        let start_ns = crate::now_ns();
        let found = self
            .store
            .lookup(&self.key, self.check)
            .map_err(&store_error)?;
        if let Some(entry) = found {
            let same_outputs = entry.outputs.len() == outputs.len()
                && outputs
                    .iter()
                    .all(|(name, _)| entry.outputs.contains_key(*name));
            if same_outputs {
                return self.recorded(start_ns, entry.outputs, false, Report::default());
            }
        }
        // One maker of a key at a time, in this store: a second waits, and
        // then finds what the first recorded.
        let _making = self.store.making(&self.key).map_err(&store_error)?;
        if let Some(entry) = self
            .store
            .lookup(&self.key, self.check)
            .map_err(&store_error)?
            .filter(|entry| {
                entry.outputs.len() == outputs.len()
                    && outputs
                        .iter()
                        .all(|(name, _)| entry.outputs.contains_key(*name))
            })
        {
            return self.recorded(start_ns, entry.outputs, false, Report::default());
        }
        let scratch = match self.ram_work {
            Some(default_bytes) => {
                let expected = self.expected(default_bytes);
                self.store.scratch_for(expected)
            }
            None => self.store.scratch(),
        }
        .map_err(&store_error)?;
        let mut work = Work {
            dir: scratch.path(),
            cx: self.cx,
            name: self.name,
            key: self.key,
            logs: &self.logs,
            report: Report::default(),
        };
        let load_start = crate::log::load_now();
        let cpu_start = crate::log::thread_cpu_ns();
        let result = body(&mut work);
        let mut report = work.report;
        report.own_cpu_ns += crate::log::thread_cpu_ns().saturating_sub(cpu_start);
        report.load = load_start.zip(crate::log::load_now()).map(|(a, b)| [a, b]);
        if let Err(error) = result {
            // The record says the body ran and failed, with what it measured
            // before it failed; the step's own error is what goes up.
            let failed = Report {
                failed: true,
                ..report
            };
            // What it took before it failed: a run that ran out of room
            // raises what the next run expects (see `expected_work_in`).
            let work_bytes = self
                .ram_work
                .map(|_| gaia_store::used_bytes(scratch.path()));
            let _ = self.write_record(start_ns, BTreeMap::new(), true, failed, work_bytes);
            return Err(self.logged(error));
        }
        // Measured before the commit moves or copies the outputs out.
        let work_bytes = self
            .ram_work
            .map(|_| gaia_store::used_bytes(scratch.path()));
        let entry = scratch
            .commit_step(&self.key, outputs)
            .map_err(&store_error)?;
        self.recorded_with(start_ns, entry.outputs, true, report, work_bytes)
    }

    /// The work directory size to expect: the last one a record of this step
    /// has, else `default_bytes`.
    fn expected_work(&self, default_bytes: u64) -> u64 {
        expected_work_in(&self.logs, self.name, default_bytes)
    }

    /// What the work directory is chosen for: the records' size, or the
    /// floor when that is more (see [`Step::ram_floor`]).
    fn expected(&self, default_bytes: u64) -> u64 {
        self.expected_work(default_bytes).max(self.ram_floor)
    }

    /// A failure of the body's own code that a fault record cannot hold,
    /// with its whole message in a run log of its own. Daedalus keeps
    /// [`REASON_BYTES`] of a fault's reason, so a check that lists what it
    /// found (config lines that did not take) would lose the list; the log
    /// keeps it, and the failure names the log. A message that fits is
    /// returned as it is, and so is a tool's failure, which has its log.
    fn logged(&self, error: StepError) -> StepError {
        let message = match error.kind() {
            ErrorKind::Body(why)
                if error.log().is_none()
                    && (why.contains('\n')
                        || self.name.len() + ": ".len() + why.len() > REASON_BYTES) =>
            {
                why.clone()
            }
            _ => return error,
        };
        let Ok(log) = RunLog::create(&self.logs, self.name, &self.key, "(the step's own check)")
        else {
            return error;
        };
        for line in message.lines() {
            log.line("err", line);
        }
        log.end("failed");
        error.with_log(log.path.clone())
    }

    /// Adds this look-up-or-make to the step record and returns it.
    fn recorded(
        &self,
        start_ns: u128,
        outputs: BTreeMap<String, ObjectRef>,
        made: bool,
        report: Report,
    ) -> Result<Made, StepError> {
        self.recorded_with(start_ns, outputs, made, report, None)
    }

    /// [`Self::recorded`], with the size the work directory had.
    fn recorded_with(
        &self,
        start_ns: u128,
        outputs: BTreeMap<String, ObjectRef>,
        made: bool,
        report: Report,
        work_bytes: Option<u64>,
    ) -> Result<Made, StepError> {
        let record = self
            .write_record(start_ns, outputs, made, report, work_bytes)
            .map_err(|error| StepError::body(self.name, format!("step record: {error}")))?;
        Ok(Made {
            outputs: record.outputs,
            made,
        })
    }

    /// Appends this look-up-or-make to the step record, as it ended now.
    fn write_record(
        &self,
        start_ns: u128,
        outputs: BTreeMap<String, ObjectRef>,
        made: bool,
        report: Report,
        work_bytes: Option<u64>,
    ) -> std::io::Result<crate::StepRecord> {
        let report = Report {
            size_measure: work_bytes.map(|_| crate::log::SIZE_MEASURE),
            ..report
        };
        let record = crate::StepRecord {
            start_ns,
            end_ns: crate::now_ns(),
            pid: std::process::id(),
            made,
            step: self.name.to_string(),
            key: self.key,
            outputs,
            work_bytes,
            report,
        };
        crate::log::record_step(&self.logs, &record)?;
        Ok(record)
    }
}

/// The work directory size of the step `name` to expect, from the step
/// records in `logs`: the last run that succeeded and recorded a size, else
/// `default_bytes`; raised to the largest size a failed run recorded since.
/// A failed run measured what it used before it stopped, so it says at least
/// that much is needed, and a run that ran out of room says it needs more
/// than it had. It never lowers what the last success measured. Only sizes
/// measured the way they are now count (see [`crate::log::SIZE_MEASURE`]);
/// a record's time and outcome are kept whatever its size measure.
fn expected_work_in(logs: &Path, name: &str, default_bytes: u64) -> u64 {
    let step = crate::log::file_name(name);
    let mut failed_since = 0;
    for record in crate::log::step_records(logs).iter().rev() {
        let current = record.report.size_measure >= Some(crate::log::SIZE_MEASURE);
        let (true, true, Some(bytes)) = (record.step == step, current, record.work_bytes) else {
            continue;
        };
        if record.report.failed {
            failed_since = failed_since.max(bytes);
        } else {
            return bytes.max(failed_since);
        }
    }
    default_bytes.max(failed_since)
}

/// What a step's body works with: its own output directory, the run's
/// cancel word and progress cells, and a way to run one tool.
pub struct Work<'a> {
    dir: &'a Path,
    cx: &'a Ctx,
    name: &'a str,
    key: StepKey,
    logs: &'a Path,
    report: Report,
}

impl std::fmt::Debug for Work<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Work")
            .field("dir", &self.dir)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Work<'_> {
    /// The step's output directory. The body writes only here.
    pub fn dir(&self) -> &Path {
        self.dir
    }

    pub fn step_name(&self) -> &str {
        self.name
    }

    /// Whether the run has been asked to stop.
    pub fn cancelled(&self) -> bool {
        self.cx.cancelled()
    }

    /// Reports how far the step has got, into the step's Daedalus progress
    /// cells.
    pub fn progress(&self, done: u32, total: u32) {
        self.cx.progress(done, total);
    }

    /// Runs `tool` to the end. It starts in the output directory unless it
    /// names another, sees only the environment it lists, and is killed
    /// with its whole process group when the run is cancelled. Everything
    /// it prints goes to a run log of its own ([`ToolOutput::log`],
    /// [`StepError::log`]). Its CPU time is added to the step's record,
    /// whether it succeeds or not.
    pub fn run(&mut self, tool: &Tool) -> Result<ToolOutput, StepError> {
        tool::run(
            tool,
            self.cx,
            self.name,
            self.dir,
            self.logs,
            &self.key,
            &mut self.report.cpu_ns,
        )
    }

    /// Adds CPU time a thread the body started used, in nanoseconds, to the
    /// step's own (see [`Report::own_cpu_ns`]). The body's own thread is
    /// counted without it; a thread measures itself with
    /// [`crate::thread_cpu_ns`] at its start and end.
    pub fn add_own_cpu(&mut self, ns: u64) {
        self.report.own_cpu_ns += ns;
    }

    /// The job count this step was given, for its record: the `-j` of a
    /// make, say.
    pub fn jobs(&mut self, jobs: u32) {
        self.report.jobs = Some(jobs);
    }

    /// Adds `value` to the step's record under `name`, replacing an earlier
    /// detail of that name. The record keeps it whether the body succeeds
    /// or fails, so a failed body can report what it got through.
    pub fn detail(&mut self, name: &str, value: serde_json::Value) {
        self.report.details.insert(name.to_string(), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StepRecord;
    use gaia_store::Digest;

    fn record(step: &str, work_bytes: Option<u64>) -> StepRecord {
        StepRecord {
            start_ns: 1,
            end_ns: 2,
            pid: 3,
            made: true,
            step: step.to_string(),
            key: StepKey(Digest::of_bytes(step.as_bytes())),
            outputs: BTreeMap::new(),
            work_bytes,
            report: crate::Report {
                size_measure: work_bytes.map(|_| crate::log::SIZE_MEASURE),
                ..crate::Report::default()
            },
        }
    }

    fn append(logs: &Path, record: &StepRecord) {
        crate::log::record_step(logs, record).expect("record");
    }

    #[test]
    fn with_no_record_the_default_is_expected() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(expected_work_in(dir.path(), "gaia.os.buildroot", 24), 24);
    }

    #[test]
    fn the_last_recorded_size_of_the_same_step_is_expected() {
        let dir = tempfile::tempdir().expect("tempdir");
        append(dir.path(), &record("gaia.os.buildroot", Some(10)));
        append(dir.path(), &record("gaia.os.buildroot", Some(20)));
        append(dir.path(), &record("gaia.os.buildroot.host", Some(99)));
        append(dir.path(), &record("gaia.os.buildroot", None));
        assert_eq!(expected_work_in(dir.path(), "gaia.os.buildroot", 24), 20);
        assert_eq!(
            expected_work_in(dir.path(), "gaia.os.buildroot.host", 8),
            99
        );
    }

    fn failed(step: &str, work_bytes: u64) -> StepRecord {
        let sized = record(step, Some(work_bytes));
        StepRecord {
            report: crate::Report {
                failed: true,
                ..sized.report.clone()
            },
            ..sized
        }
    }

    /// A failed record written before sizes had a measure: 143 GB, read off
    /// a plant of hard links counted once per link.
    fn unmeasured_failed(step: &str, work_bytes: u64) -> StepRecord {
        let mut old = failed(step, work_bytes);
        old.report.size_measure = None;
        old
    }

    #[test]
    fn a_size_from_before_the_measure_is_not_expected() {
        let big = 143_094_362_112;
        let dir = tempfile::tempdir().expect("tempdir");
        append(dir.path(), &unmeasured_failed("gaia.os.buildroot", big));
        // As for a new step: the default, under the 60G RAM cap, so a forced
        // RAM run is not refused on it.
        assert_eq!(expected_work_in(dir.path(), "gaia.os.buildroot", 24), 24);
        let older = tempfile::tempdir().expect("tempdir");
        let mut one = unmeasured_failed("gaia.os.buildroot", big);
        one.report.size_measure = Some(crate::log::SIZE_MEASURE - 1);
        append(older.path(), &one);
        assert_eq!(expected_work_in(older.path(), "gaia.os.buildroot", 24), 24);
        // Its time and outcome are still read.
        let read = crate::step_records(dir.path());
        assert!(read[0].report.failed);
        assert_eq!(read[0].work_bytes, Some(big));
    }

    #[test]
    fn a_measured_failed_size_is_still_expected_however_big() {
        let big = 143_094_362_112;
        let dir = tempfile::tempdir().expect("tempdir");
        append(dir.path(), &unmeasured_failed("gaia.os.buildroot", 7));
        append(dir.path(), &failed("gaia.os.buildroot", big));
        assert_eq!(expected_work_in(dir.path(), "gaia.os.buildroot", 24), big);
    }

    #[test]
    fn a_failed_run_that_used_more_raises_what_is_expected() {
        let dir = tempfile::tempdir().expect("tempdir");
        append(dir.path(), &record("gaia.os.buildroot", Some(10)));
        append(dir.path(), &failed("gaia.os.buildroot", 50));
        append(dir.path(), &failed("gaia.os.buildroot", 30));
        assert_eq!(expected_work_in(dir.path(), "gaia.os.buildroot", 24), 50);
        // With no success at all, past the default too.
        let none = tempfile::tempdir().expect("tempdir");
        append(none.path(), &failed("gaia.os.buildroot", 40));
        assert_eq!(expected_work_in(none.path(), "gaia.os.buildroot", 24), 40);
    }

    #[test]
    fn a_failed_run_that_used_less_does_not_lower_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        append(dir.path(), &failed("gaia.os.buildroot", 90));
        append(dir.path(), &record("gaia.os.buildroot", Some(20)));
        append(dir.path(), &failed("gaia.os.buildroot", 5));
        // The failure before the last success is past: that success measured.
        assert_eq!(expected_work_in(dir.path(), "gaia.os.buildroot", 24), 20);
    }

    #[test]
    fn a_failed_run_records_its_size_and_the_floor_is_expected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path().join("store")).expect("store");
        let cx = Ctx::new();
        let key = StepKey(Digest::of_bytes(b"floor"));
        let result = Step::new(&store, &cx, "gaia.test.floor", key)
            .ram_work(1)
            .look_up_or_make(&[("out", Path::new("out"))], |work| {
                std::fs::write(work.dir().join("used"), vec![0_u8; 1 << 20])
                    .map_err(|e| StepError::body("gaia.test.floor", e))?;
                Err(StepError::body("gaia.test.floor", "planted failure"))
            });
        assert!(result.is_err());
        let logs = crate::logs_dir(&store);
        let records = crate::step_records(&logs);
        let last = records.last().expect("a record");
        assert!(last.report.failed);
        assert!(last.work_bytes.expect("a size") >= 1 << 20);
        assert_eq!(last.report.size_measure, Some(crate::log::SIZE_MEASURE));
        assert!(expected_work_in(&logs, "gaia.test.floor", 1) >= 1 << 20);
        let step = Step::new(&store, &cx, "gaia.test.floor", key)
            .ram_work(1)
            .ram_floor(7 << 40);
        assert_eq!(step.expected(1), 7 << 40, "the floor, when it is more");
        let low = Step::new(&store, &cx, "gaia.test.floor", key)
            .ram_work(1)
            .ram_floor(1);
        assert!(
            low.expected(1) >= 1 << 20,
            "the records, when they are more"
        );
    }

    /// Spends about `ms` of CPU on the calling thread.
    fn spin(ms: u64) -> u64 {
        let start = crate::thread_cpu_ns();
        let mut x = 1_u64;
        while crate::thread_cpu_ns() - start < ms * 1_000_000 {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        }
        x
    }

    /// Work done in Gaia's own process is in the record: the body's thread
    /// by itself, a thread it starts when it reports, and the machine's load
    /// at the start and end.
    #[test]
    fn the_steps_own_cpu_and_the_load_are_recorded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path().join("store")).expect("store");
        let cx = Ctx::new();
        let key = StepKey(Digest::of_bytes(b"own cpu"));
        Step::new(&store, &cx, "gaia.test.own-cpu", key)
            .look_up_or_make(&[("out", Path::new("out"))], |work| {
                std::hint::black_box(spin(60));
                let worker = std::thread::spawn(|| {
                    std::hint::black_box(spin(80));
                    crate::thread_cpu_ns()
                })
                .join()
                .expect("worker");
                work.add_own_cpu(worker);
                std::fs::write(work.dir().join("out"), b"made")
                    .map_err(|e| StepError::body("gaia.test.own-cpu", e))
            })
            .expect("made");
        let records = crate::step_records(&crate::logs_dir(&store));
        let report = &records.last().expect("a record").report;
        assert!(
            report.own_cpu_ns >= 140_000_000,
            "the body's 60 ms and the worker's 80 ms: {} ns",
            report.own_cpu_ns
        );
        assert_eq!(report.cpu_ns, 0, "no tool ran");
        let [start, end] = report.load.expect("the load");
        assert!(start >= 0.0 && end >= 0.0);
    }

    #[test]
    fn a_record_with_a_size_round_trips_and_one_without_still_parses() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sized = record("gaia.os.buildroot", Some(7 << 30));
        let plain = record("gaia.os.buildroot", None);
        append(dir.path(), &sized);
        append(dir.path(), &plain);
        let read = crate::step_records(dir.path());
        assert_eq!(read, vec![sized, plain]);
    }

    #[test]
    fn an_old_line_without_a_size_has_none() {
        let line = format!("1 2 3 made gaia.os.buildroot {}\n", Digest::of_bytes(b"k"));
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("steps.log"), line).expect("write");
        let read = crate::step_records(dir.path());
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].work_bytes, None);
    }
}
