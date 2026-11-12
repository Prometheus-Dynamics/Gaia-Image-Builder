//! What the check nodes share: reading a requirements input, opening the
//! tree, and the verdict. A check that finds problems fails its step, and
//! the failure names every problem.

use gaia_step::{StepError, Store};
use gaia_types::Tree;
use serde::de::DeserializeOwned;
use std::path::PathBuf;

/// A node's requirements, from its JSON text input. Empty text means the
/// defaults. An unknown key or a wrong type is an error that names it.
pub fn parse_requirements<T: DeserializeOwned + Default>(
    step: &str,
    text: &str,
) -> Result<T, StepError> {
    let text = if text.trim().is_empty() { "{}" } else { text };
    serde_json::from_str(text).map_err(|e| StepError::body(step, format!("requirements: {e}")))
}

/// Where the tree is in the store.
pub fn root(step: &str, store: &str, tree: &Tree) -> Result<PathBuf, StepError> {
    let store = Store::open(store).map_err(StepError::store(step))?;
    tree.object
        .path_in(&store)
        .ok_or_else(|| StepError::body(step, "a tree that is not in the store"))
}

/// Fails the step, naming every problem, when there is any.
pub fn verdict(step: &str, problems: Vec<String>) -> Result<(), StepError> {
    if problems.is_empty() {
        return Ok(());
    }
    let mut why = format!("{} problem(s) in the image:", problems.len());
    for problem in &problems {
        why.push_str("\n  - ");
        why.push_str(problem);
    }
    Err(StepError::body(step, why))
}

/// A check that could not run at all (a tree that cannot be read) fails its
/// step with what it could not read.
pub fn unreadable(step: &str, why: impl std::fmt::Display) -> StepError {
    StepError::body(step, format!("the image cannot be read: {why}"))
}
