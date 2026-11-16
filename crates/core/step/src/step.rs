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
                let expected = self.expected_work(default_bytes);
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
        let result = body(&mut work);
        let report = work.report;
        if let Err(error) = result {
            // The record says the body ran and failed, with what it measured
            // before it failed; the step's own error is what goes up.
            let failed = Report {
                failed: true,
                ..report
            };
            let _ = self.write_record(start_ns, BTreeMap::new(), true, failed, None);
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
/// records in `logs`: the last one that recorded a size, else `default_bytes`.
fn expected_work_in(logs: &Path, name: &str, default_bytes: u64) -> u64 {
    let step = crate::log::file_name(name);
    crate::log::step_records(logs)
        .iter()
        .rev()
        .find(|record| record.step == step && record.work_bytes.is_some())
        .and_then(|record| record.work_bytes)
        .unwrap_or(default_bytes)
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
            report: crate::Report::default(),
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
