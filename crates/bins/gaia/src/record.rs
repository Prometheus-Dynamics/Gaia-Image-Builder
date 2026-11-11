//! What one `gaia build` did, kept as `runs/<start ns>.json` next to the
//! store. `show` reads the latest; `clean` keeps what the latest N name.

use crate::Stop;
use gaia_store::{Keep, ObjectRef, StepKey, Store};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub product: PathBuf,
    pub start_ns: u64,
    pub end_ns: u64,
    /// `done`, `failed` or `cancelled`.
    pub outcome: String,
    /// By host port, `<image>.<output>`.
    pub outputs: BTreeMap<String, Output>,
    /// Every step this run looked up or made, in the order they ended.
    pub steps: Vec<Step>,
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
    pub ms: u64,
    pub outputs: BTreeMap<String, ObjectRef>,
}

/// The run records directory of `store`: `runs` next to its root.
pub fn runs_dir(store: &Store) -> PathBuf {
    store.root().with_file_name("runs")
}

impl Run {
    pub fn write(&self, store: &Store) -> Result<PathBuf, Stop> {
        let dir = runs_dir(store);
        std::fs::create_dir_all(&dir)
            .map_err(|e| Stop::usage(format!("{}: {e}", dir.display())))?;
        let path = dir.join(format!("{}.json", self.start_ns));
        let text = serde_json::to_string_pretty(self).map_err(Stop::usage)?;
        std::fs::write(&path, text).map_err(|e| Stop::usage(format!("{}: {e}", path.display())))?;
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
