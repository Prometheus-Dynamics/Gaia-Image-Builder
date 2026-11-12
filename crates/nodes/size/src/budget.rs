//! `gaia.size.budget`: an image's total, partitions and components against
//! the budget, and the largest contributors named when one is over.

use daedalus::prelude::*;
use gaia_describe::SizeText;
use gaia_step::{Ctx, StepError, Store};
use gaia_types::Owned;
use gaia_types::{Artifact, Image, Tree};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

const NAME: &str = "gaia.size.budget";
/// How many largest contributors a failure names.
const NAMED: usize = 5;

/// What the product allows. Every key is optional; a key the budget does
/// not set is not checked.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Budget {
    /// The image's total size.
    pub total: Option<SizeText>,
    /// Each partition's size, by the partition's name.
    pub partitions: BTreeMap<String, SizeText>,
    /// Each component's size, by the component's name.
    pub components: BTreeMap<String, SizeText>,
}

/// The sizes a budget is checked against: the image's total, each of its
/// partitions, and each component's bytes in the merged tree.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Sizes {
    pub total: u64,
    pub partitions: BTreeMap<String, u64>,
    pub components: BTreeMap<String, u64>,
}

impl Sizes {
    /// The image's sizes: its total, its partitions, and the bytes of the
    /// regular files each component owns under `root`. A file with several
    /// owners counts for the first one, so nothing is counted twice.
    pub fn of(image: &Image, root: &Path, owned: &[Owned]) -> Self {
        let mut components: BTreeMap<String, u64> = BTreeMap::new();
        for entry in owned {
            let Some(first) = entry.owners.first() else {
                continue;
            };
            let bytes = file_bytes(root, &entry.path);
            *components.entry(first.clone()).or_insert(0) += bytes;
        }
        Self {
            total: image.size.0,
            partitions: image
                .partitions
                .iter()
                .map(|partition| (partition.name.clone(), partition.size.0))
                .collect(),
            components,
        }
    }

    /// The largest contributors, biggest first: each partition and component
    /// with its bytes, at most [`NAMED`].
    pub fn largest(&self) -> Vec<(String, u64)> {
        let mut items: Vec<(String, u64)> = self
            .partitions
            .iter()
            .map(|(name, bytes)| (format!("partition {name}"), *bytes))
            .chain(
                self.components
                    .iter()
                    .map(|(name, bytes)| (format!("component {name}"), *bytes)),
            )
            .filter(|(_, bytes)| *bytes > 0)
            .collect();
        items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        items.truncate(NAMED);
        items
    }
}

/// The image, unchanged, when every item the budget names is within it. An
/// item over its budget fails the step, naming each one and the largest
/// contributors.
#[node(
    id = "gaia.size.budget",
    inputs(store, image, tree, owners, budget),
    outputs(image)
)]
pub fn size_budget(
    store: &str,
    image: Image,
    tree: Tree,
    owners: Artifact,
    budget: &str,
    _cx: &Ctx,
) -> Result<Image, StepError> {
    let budget = parse_budget(budget)?;
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let root = tree
        .object
        .path_in(&store)
        .ok_or_else(|| StepError::body(NAME, "a tree that is not in the store"))?;
    let owners_path = owners
        .object
        .path_in(&store)
        .ok_or_else(|| StepError::body(NAME, "owners.json is not in the store"))?;
    let text = std::fs::read(&owners_path)
        .map_err(|e| StepError::body(NAME, format!("owners.json: {e}")))?;
    let owned: Vec<Owned> = serde_json::from_slice(&text)
        .map_err(|e| StepError::body(NAME, format!("owners.json: {e}")))?;

    let sizes = Sizes::of(&image, &root, &owned);
    let problems = problems(&sizes, &budget);
    if problems.is_empty() {
        return Ok(image);
    }
    Err(StepError::body(NAME, report(&problems, &sizes)))
}

/// Every item over its budget, and every name the budget gives that the image
/// does not have, as one line each.
pub fn problems(sizes: &Sizes, budget: &Budget) -> Vec<String> {
    let mut problems = Vec::new();
    if let Some(limit) = &budget.total {
        let limit = limit.0.0;
        if let Some(over) = over(sizes.total, limit) {
            problems.push(format!(
                "the image is {} bytes, over its budget of {limit} by {over}",
                sizes.total
            ));
        }
    }
    for (name, limit) in &budget.partitions {
        let limit = limit.0.0;
        match sizes.partitions.get(name) {
            None => problems.push(format!(
                "the budget names partition {name}, which the image does not have"
            )),
            Some(size) => {
                if let Some(over) = over(*size, limit) {
                    problems.push(format!(
                        "partition {name} is {size} bytes, over its budget of {limit} by {over}"
                    ));
                }
            }
        }
    }
    for (name, limit) in &budget.components {
        let limit = limit.0.0;
        let size = sizes.components.get(name).copied().unwrap_or(0);
        if let Some(over) = over(size, limit) {
            problems.push(format!(
                "component {name} is {size} bytes, over its budget of {limit} by {over}"
            ));
        }
    }
    problems
}

/// How far `size` is over `limit`, when it is.
fn over(size: u64, limit: u64) -> Option<u64> {
    (size > limit).then(|| size - limit)
}

/// The failure text: each problem, then the largest contributors.
fn report(problems: &[String], sizes: &Sizes) -> String {
    let mut text = format!("{} over budget:", problems.len());
    for problem in problems {
        let _ = write!(text, "\n  - {problem}");
    }
    text.push_str("\nlargest contributors:");
    for (label, bytes) in sizes.largest() {
        let _ = write!(text, "\n  - {label}: {bytes} bytes");
    }
    text
}

/// The size of the regular file at `path` under `root`, or 0 when there is
/// none (a directory, a link, or nothing there).
fn file_bytes(root: &Path, path: &str) -> u64 {
    match std::fs::symlink_metadata(root.join(path.trim_start_matches('/'))) {
        Ok(meta) if meta.is_file() => meta.len(),
        _ => 0,
    }
}

/// The budget from its JSON text. Empty text is no budget at all.
fn parse_budget(text: &str) -> Result<Budget, StepError> {
    let text = if text.trim().is_empty() { "{}" } else { text };
    serde_json::from_str(text).map_err(|e| StepError::body(NAME, format!("budget: {e}")))
}
