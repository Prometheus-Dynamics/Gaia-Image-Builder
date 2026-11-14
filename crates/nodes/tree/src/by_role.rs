//! `gaia.tree.by-role`: a directory filled from a layout its reader owns.
//!
//! A boot firmware, an installer and a check at every boot all have to
//! agree on what a slot's boot directory holds and under which names. That
//! list belongs to the program that reads the directory, so it is read
//! from that program's source at the locked commit, and a build types no
//! file name: it has each file by its *role* (`kernel`, `device-tree`,
//! `cmdline`), and the layout says where a role goes. A file the reader's
//! own generator writes is taken from what the generator wrote, at the
//! layout's path.
//!
//! The layout, TOML, `version = 3`:
//!
//! ```toml
//! version = 3
//! [[file]]
//! name = "kernel8.img"        # the path in the directory
//! role = "kernel"             # what it is
//! source = "build"            # the build has it by role; or "generated"
//! firmware = ["rpi-tryboot"]  # the kinds of boot it is for
//! required = true
//!
//! [[record]]                  # a file at the root of the boot partition,
//! name = "trial.rec"          # beside the slots' directories, that the
//! role = "trial"              # generator pre-creates at a fixed size
//! firmware = ["rpi-tryboot"]
//! bytes = 1024
//! source = "generated"
//! ```
//!
//! The records are not part of a slot's directory, so this step does not
//! place them; it reads them, refuses what it does not know in them as in
//! the rest, and [`records`] gives them to the step that makes the boot
//! partition.
//!
//! Another version, or a key this reader does not know, is refused: it
//! means a newer file.

use crate::walk::{copy, inside, list, make_dirs};
use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::Tree;
use serde::Deserialize;
use std::path::Path;

const NAME: &str = "gaia.tree.by-role";
/// The version this reader knows.
const VERSION: i64 = 3;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Layout {
    #[allow(dead_code)]
    version: i64,
    file: Vec<Entry>,
    record: Vec<RecordEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordEntry {
    name: String,
    role: RecordRole,
    firmware: Vec<String>,
    bytes: u64,
    source: Source,
}

/// What a record file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordRole {
    Trial,
    Reason,
    Episodes,
}

/// One record file at the root of the boot partition, for a firmware.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// Its name at the root of the boot partition.
    pub name: String,
    pub role: RecordRole,
    /// Its exact size.
    pub bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    role: String,
    source: Source,
    firmware: Vec<String>,
    required: bool,
}

/// Where a file of the directory comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The build has it, by role.
    Build,
    /// The layout's owner's generator wrote it, at the layout's path.
    Generated,
}

/// One file of the directory for a firmware.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub role: String,
    /// The path in the directory.
    pub path: String,
    pub source: Source,
    pub required: bool,
}

/// The layout `text` (named `file` in errors), read: this version, and no
/// key this reader does not know.
fn read(text: &str, file: &str) -> Result<Layout, String> {
    let value: toml::Value =
        toml::from_str(text).map_err(|e| format!("{file}: {}", e.message()))?;
    match value.get("version").and_then(toml::Value::as_integer) {
        Some(VERSION) => {}
        Some(other) => {
            return Err(format!(
                "{file}: version {other}; this build reads version {VERSION}"
            ));
        }
        None => return Err(format!("{file}: no `version`")),
    }
    value
        .try_into()
        .map_err(|e: toml::de::Error| format!("{file}: {}", e.message()))
}

/// The record files the layout `text` lists for `firmware`, in its order:
/// files at the root of the boot partition that the layout's owner's
/// generator pre-creates.
///
/// # Errors
///
/// As [`placements`]; and a record that is not the generator's, has no
/// size, or has a name that is not one file name or is listed twice.
pub fn records(text: &str, file: &str, firmware: &str) -> Result<Vec<Record>, String> {
    let layout = read(text, file)?;
    let mut found: Vec<Record> = Vec::new();
    for entry in &layout.record {
        if !entry.firmware.iter().any(|kind| kind == firmware) {
            continue;
        }
        let name = inside(&entry.name).map_err(|why| format!("{file}: {why}"))?;
        if name.contains('/') {
            return Err(format!(
                "{file}: the record {name} is not at the root of the boot partition"
            ));
        }
        if entry.source != Source::Generated {
            return Err(format!(
                "{file}: the record {name} is not the generator's; a build has no record to supply"
            ));
        }
        if entry.bytes == 0 {
            return Err(format!("{file}: the record {name} has no size"));
        }
        if found
            .iter()
            .any(|other| other.name == name || other.role == entry.role)
        {
            return Err(format!(
                "{file}: the record {name} is listed twice for {firmware}, by name or by role"
            ));
        }
        found.push(Record {
            name,
            role: entry.role,
            bytes: entry.bytes,
        });
    }
    Ok(found)
}

/// Where each role goes for `firmware`, by the layout `text` (named `file`
/// in errors), in the layout's order.
///
/// # Errors
///
/// A layout of another version, with an unknown key, with a path that
/// leaves the directory, or with a role or a path listed twice for the
/// firmware.
pub fn placements(text: &str, file: &str, firmware: &str) -> Result<Vec<Placed>, String> {
    let layout = read(text, file)?;
    let mut found: Vec<Placed> = Vec::new();
    for entry in &layout.file {
        if !entry.firmware.iter().any(|kind| kind == firmware) {
            continue;
        }
        let path = inside(&entry.name).map_err(|why| format!("{file}: {why}"))?;
        if entry.role.is_empty() || entry.role.contains('/') {
            return Err(format!(
                "{file}: {:?} is not a role (a name with no `/`)",
                entry.role
            ));
        }
        if let Some(other) = found
            .iter()
            .find(|other| other.role == entry.role || other.path == path)
        {
            return Err(format!(
                "{file}: {} as {} and {} as {path} cannot both be listed for {firmware}",
                other.role, other.path, entry.role
            ));
        }
        found.push(Placed {
            role: entry.role.clone(),
            path,
            source: entry.source,
            required: entry.required,
        });
    }
    if found.is_empty() {
        let mut kinds: Vec<&str> = layout
            .file
            .iter()
            .flat_map(|entry| entry.firmware.iter().map(String::as_str))
            .collect();
        kinds.sort_unstable();
        kinds.dedup();
        return Err(format!(
            "{file}: no file is listed for {firmware}; it lists {}",
            kinds.join(", ")
        ));
    }
    Ok(found)
}

/// The directory the layout describes for `firmware`. A file whose source
/// is the build comes from `by_role`, a tree whose top-level files are
/// named by role; one whose source is the generator comes from `generated`,
/// at the layout's own path. The layout is the file `layout_path` of the
/// tree `layout`.
///
/// A required file with nothing to fill it fails, naming it. So does a
/// file in `by_role` whose name is no role the build is to supply: the
/// build made something the reader of the directory does not know about.
/// What else `generated` holds is the generator's business and is left.
#[node(
    id = "gaia.tree.by-role",
    inputs(store, layout, layout_path, firmware, by_role, generated),
    outputs(tree)
)]
pub fn tree_by_role(
    store: &str,
    layout: Tree,
    layout_path: &str,
    firmware: &str,
    by_role: Tree,
    generated: Tree,
    cx: &Ctx,
) -> Result<Tree, StepError> {
    let fail = |why: String| StepError::body(NAME, why);
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let in_store = |tree: &Tree, what: &str| {
        tree.object
            .object_ref()
            .ok_or_else(|| fail(format!("{what} is not in the store")))
    };
    let layout_object = in_store(&layout, "the layout's tree")?;
    let roles_object = in_store(&by_role, "the tree of files by role")?;
    let generated_object = in_store(&generated, "the generated tree")?;
    let written = store.path(&generated_object);
    let relative = inside(layout_path).map_err(fail)?;
    let text = std::fs::read_to_string(store.path(&layout_object).join(&relative))
        .map_err(|e| fail(format!("the layout {relative}: {e}")))?;
    let wanted = placements(&text, &relative, firmware).map_err(fail)?;
    let roles = store.path(&roles_object);
    let have = list(&roles).map_err(|e| fail(e.to_string()))?;

    // Where each file of the directory is taken from.
    let from_of = |placed: &Placed| match placed.source {
        Source::Build => roles.join(&placed.role),
        Source::Generated => written.join(&placed.path),
    };
    let mut missing = Vec::new();
    for placed in &wanted {
        if placed.required && !from_of(placed).is_file() {
            missing.push(match placed.source {
                Source::Build => format!(
                    "{}: the build has no file for this role (it goes to {})",
                    placed.role, placed.path
                ),
                Source::Generated => {
                    format!("{}: the generator wrote no {}", placed.role, placed.path)
                }
            });
        }
    }
    if !missing.is_empty() {
        return Err(fail(format!(
            "the layout {relative} requires for {firmware} what is not there:\n  {}",
            missing.join("\n  ")
        )));
    }
    let supplied = |name: &str| {
        wanted
            .iter()
            .any(|placed| placed.source == Source::Build && placed.role == name)
    };
    let unknown: Vec<&str> = have
        .keys()
        .filter(|name| !name.is_empty() && !supplied(name))
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        return Err(fail(format!(
            "the layout {relative} has no role {} for the build to supply for {firmware}; those roles are {}",
            unknown.join(", "),
            wanted
                .iter()
                .filter(|placed| placed.source == Source::Build)
                .map(|placed| placed.role.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    // The layout as this firmware reads it, not its whole text: a line for
    // another firmware changes nothing here.
    // And of the generator's tree only the files taken, by content: what
    // else it wrote changes nothing here.
    let mut plan = Vec::new();
    for placed in &wanted {
        let taken = match placed.source {
            Source::Build => "build".to_string(),
            Source::Generated => match std::fs::read(from_of(placed)) {
                Ok(bytes) => format!("generated {}", gaia_step::Digest::of_bytes(&bytes)),
                Err(_) => "generated, absent".to_string(),
            },
        };
        plan.push(format!(
            "{}\t{}\t{}\t{taken}",
            placed.role, placed.path, placed.required
        ));
    }
    let key = KeyBuilder::new(NAME, 2)
        .config("firmware", firmware)
        .config("placements", plan.join("\n"))
        .input("by-role", roles_object.digest)
        .finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("tree", Path::new("tree"))],
        |work| {
            let io = |e: std::io::Error| StepError::body(NAME, e.to_string());
            let out = work.dir().join("tree");
            make_dirs(&out).map_err(io)?;
            for placed in &wanted {
                let from = from_of(placed);
                if !from.is_file() {
                    continue;
                }
                let to = out.join(&placed.path);
                if let Some(parent) = to.parent() {
                    make_dirs(parent).map_err(io)?;
                }
                copy(&from, &to).map_err(io)?;
            }
            Ok(())
        },
    )?;
    Ok(Tree {
        object: made.outputs["tree"].into(),
        entries: Vec::new(),
    })
}
