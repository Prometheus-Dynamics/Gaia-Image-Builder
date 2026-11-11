//! Look up or make.

use crate::{Ctx, StepError, Tool, ToolOutput, tool};
use gaia_store::{Check, ObjectRef, StepKey, Store};
use std::collections::BTreeMap;
use std::path::Path;

/// One run of one step: where its outputs go, how it is cancelled, and its
/// key.
pub struct Step<'a> {
    store: &'a Store,
    cx: &'a Ctx,
    name: &'a str,
    key: StepKey,
    check: Check,
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
        }
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
                return Ok(Made {
                    outputs: entry.outputs,
                    made: false,
                });
            }
        }
        let scratch = self.store.scratch().map_err(&store_error)?;
        let mut work = Work {
            dir: scratch.path(),
            cx: self.cx,
            name: self.name,
        };
        body(&mut work)?;
        let entry = scratch
            .commit_step(&self.key, outputs)
            .map_err(&store_error)?;
        Ok(Made {
            outputs: entry.outputs,
            made: true,
        })
    }
}

/// What a step's body works with: its own output directory, the run's
/// cancel word and progress cells, and a way to run one tool.
pub struct Work<'a> {
    dir: &'a Path,
    cx: &'a Ctx,
    name: &'a str,
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
    /// with its whole process group when the run is cancelled.
    pub fn run(&mut self, tool: &Tool) -> Result<ToolOutput, StepError> {
        tool::run(tool, self.cx, self.name, self.dir)
    }
}
