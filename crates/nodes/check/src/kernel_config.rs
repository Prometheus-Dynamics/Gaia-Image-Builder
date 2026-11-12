//! `gaia.check.kernel-config`: the kernel configuration options a board needs
//! are set the way the board needs them.
//!
//! The configuration is read from a file in the tree (the kernel's `.config`,
//! placed there by the kernel build). An option is `CONFIG_NAME=value`: the
//! file must hold that exact line. `CONFIG_NAME=n` means the file must say
//! `# CONFIG_NAME is not set`.

use crate::common::{parse_requirements, root, verdict};
use crate::tree;
use daedalus::prelude::*;
use gaia_step::{Ctx, StepError};
use gaia_types::Tree;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

const NAME: &str = "gaia.check.kernel-config";

/// The configuration file in the tree, and the options it must hold.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Requirements {
    pub file: String,
    pub options: Vec<String>,
}

/// Fails when an option is not set as required. Gives the tree unchanged.
#[node(
    id = "gaia.check.kernel-config",
    inputs(store, tree, requirements),
    outputs(tree)
)]
pub fn check_kernel_config(
    store: &str,
    tree: Tree,
    requirements: &str,
    _cx: &Ctx,
) -> Result<Tree, StepError> {
    let wanted: Requirements = parse_requirements(NAME, requirements)?;
    let dir = root(NAME, store, &tree)?;
    verdict(NAME, problems(&dir, &wanted))?;
    Ok(tree)
}

/// Every option the file does not hold as required, as one line each.
pub fn problems(root: &Path, wanted: &Requirements) -> Vec<String> {
    if wanted.options.is_empty() {
        return Vec::new();
    }
    let Some(bytes) = tree::read(root, &wanted.file) else {
        return vec![format!(
            "kernel configuration {} is not in the image, so {} options cannot be checked",
            wanted.file,
            wanted.options.len()
        )];
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut set: HashMap<&str, &str> = HashMap::new();
    let mut unset: HashSet<&str> = HashSet::new();
    for line in text.lines().map(str::trim) {
        if let Some((name, value)) = line.split_once('=')
            && name.starts_with("CONFIG_")
        {
            set.insert(name, value);
        } else if let Some(name) = line
            .strip_prefix("# ")
            .and_then(|rest| rest.strip_suffix(" is not set"))
        {
            unset.insert(name);
        }
    }

    let mut problems = Vec::new();
    for option in &wanted.options {
        let Some((name, value)) = option.split_once('=') else {
            problems.push(format!("kernel option {option} is not CONFIG_NAME=value"));
            continue;
        };
        if !name.starts_with("CONFIG_") {
            problems.push(format!(
                "kernel option {option} does not name a CONFIG_ option"
            ));
            continue;
        }
        let found = set.get(name).copied();
        let off = unset.contains(name);
        let problem = match (value, found) {
            ("n", _) if off => None,
            ("n", Some(found)) => Some(format!(
                "{name} must not be set in {}, but it is {found}",
                wanted.file
            )),
            ("n", None) => Some(format!(
                "{name} must not be set in {}, but the file does not say it is not set",
                wanted.file
            )),
            (value, Some(found)) if found == value => None,
            (value, Some(found)) => Some(format!(
                "{name} is {found} in {}, the board needs {value}",
                wanted.file
            )),
            (value, None) if off => Some(format!(
                "{name} is not set in {}, the board needs {value}",
                wanted.file
            )),
            (value, None) => Some(format!(
                "{name} is missing from {}, the board needs {value}",
                wanted.file
            )),
        };
        problems.extend(problem);
    }
    problems
}
