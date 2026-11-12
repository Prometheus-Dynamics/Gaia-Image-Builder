//! `gaia.check.modules`: the kernel modules and the firmware files a board
//! needs are in the image.
//!
//! A module counts as present when a file of that name is under `lib/modules/`
//! or `usr/lib/modules/` (`.ko`, `.ko.xz`, `.ko.zst` or `.ko.gz`), or when
//! `modules.builtin` lists it as built into the kernel. Names match as
//! modprobe matches them: `-` and `_` are the same. A firmware file counts when
//! the path reaches a regular file in the tree, with links inside the tree
//! followed. Nothing is decompressed and nothing is loaded.

use crate::common::{parse_requirements, root, unreadable, verdict};
use crate::tree::{self, Kind};
use daedalus::prelude::*;
use gaia_step::{Ctx, StepError};
use gaia_types::Tree;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::Path;

const NAME: &str = "gaia.check.modules";
const MODULE_DIRS: [&str; 2] = ["lib/modules/", "usr/lib/modules/"];
const SUFFIXES: [&str; 4] = [".ko", ".ko.xz", ".ko.zst", ".ko.gz"];

/// What the board needs: module names (as `modprobe` names them) and the
/// firmware paths in the image (`/lib/firmware/...`).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Requirements {
    pub modules: Vec<String>,
    pub firmware: Vec<String>,
}

/// Fails when a required module or firmware file is not in the tree. Gives
/// the tree unchanged.
#[node(
    id = "gaia.check.modules",
    inputs(store, tree, requirements),
    outputs(tree)
)]
pub fn check_modules(
    store: &str,
    tree: Tree,
    requirements: &str,
    _cx: &Ctx,
) -> Result<Tree, StepError> {
    let wanted: Requirements = parse_requirements(NAME, requirements)?;
    let dir = root(NAME, store, &tree)?;
    let problems = problems(&dir, &wanted).map_err(|e| unreadable(NAME, e))?;
    verdict(NAME, problems)?;
    Ok(tree)
}

/// Every missing module and firmware file, as one line each.
pub fn problems(root: &Path, wanted: &Requirements) -> Result<Vec<String>, String> {
    let mut shipped = BTreeSet::new();
    for (path, kind) in tree::walk(root)? {
        if kind != Kind::File || !in_module_dir(&path) {
            continue;
        }
        let base = path.rsplit('/').next().unwrap_or(&path);
        if base == "modules.builtin" {
            let text = std::fs::read(root.join(&path)).map_err(|e| format!("{path}: {e}"))?;
            for line in String::from_utf8_lossy(&text).lines() {
                let base = line.trim().rsplit('/').next().unwrap_or_default();
                if let Some(name) = base.strip_suffix(".ko") {
                    shipped.insert(normal(name));
                }
            }
        } else if let Some(name) = module_name(base) {
            shipped.insert(name);
        }
    }

    let mut problems = Vec::new();
    for module in &wanted.modules {
        let name = normal(module.strip_suffix(".ko").unwrap_or(module));
        if !shipped.contains(&name) {
            problems.push(format!(
                "kernel module {module} is not in the image (no file under lib/modules/ or usr/lib/modules/, and not built in)"
            ));
        }
    }
    for path in &wanted.firmware {
        if !tree::is_file(root, path.trim_start_matches('/')) {
            problems.push(format!("firmware {path} is not in the image"));
        }
    }
    Ok(problems)
}

fn in_module_dir(path: &str) -> bool {
    MODULE_DIRS.iter().any(|dir| path.starts_with(dir))
}

/// The module a file name is, when it is a module file.
fn module_name(base: &str) -> Option<String> {
    SUFFIXES
        .iter()
        .find_map(|suffix| base.strip_suffix(suffix))
        .map(normal)
}

/// The name modprobe compares: `-` and `_` are one character.
fn normal(name: &str) -> String {
    name.replace('-', "_")
}
