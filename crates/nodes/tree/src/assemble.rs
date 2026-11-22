//! `gaia.tree.assemble`: one file of the root filesystem made of several
//! owners' parts, in a stated order.
//!
//! For configuration with no `.d` directory (`/etc/mdev.conf`): the OS has
//! its file, a component cannot append to it, and two components cannot
//! both install it. Each gives its part instead, and the product says in
//! which order they are put together.

use crate::walk::{inside, make_dirs, set_mode};
use daedalus::prelude::*;
use daedalus_cpu_author::FanIn;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{FileEntry, Layer, Tree};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const NAME: &str = "gaia.tree.assemble";
/// The name in `order` of the OS's own file.
pub const OS_PART: &str = "os";
/// The owner of the assembled file in the merged tree.
pub const OWNER: &str = "assembled";
/// The file's mode when the OS has none.
const MODE: u32 = 0o644;

/// The layer that holds `path`, the parts named in `order` (a JSON list
/// of owners, `os` for the file in `base`) put together in that order:
/// each part's text as it is, a newline added after one that does not end
/// in one. `parts` (`parts0`, `parts1`, ...) are the components' layers,
/// each with its fragment at `path`; a part not named, a name with no part,
/// and an OS file left out of the order are errors.
///
/// A rule is a line neither blank nor a comment (`#`); its name is its
/// `rule_key`th whitespace-separated field, or the whole rule when
/// `rule_key` is 0. Two rules of one name are an error naming both owners.
///
/// The file has the OS's mode and owner ids when the OS has it, and the
/// layer replaces it; otherwise root's and 0644. Keyed on what it writes.
#[node(
    id = "gaia.tree.assemble",
    inputs(store, base, path, order, rule_key, parts),
    outputs(layer)
)]
pub fn tree_assemble(
    store: &str,
    base: Tree,
    path: &str,
    order: &str,
    rule_key: i64,
    parts: FanIn<'_, Layer>,
    cx: &Ctx,
) -> Result<Layer, StepError> {
    let fail = |why: String| StepError::body(NAME, why);
    let to = inside(path).map_err(fail)?;
    let order: Vec<String> = serde_json::from_str(order)
        .map_err(|e| fail(format!("the order is not a list of names: {e}")))?;
    let rule_key = usize::try_from(rule_key)
        .map_err(|_| fail(format!("rule_key {rule_key} is not a field number, or 0")))?;
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let root = base
        .object
        .path_in(&store)
        .ok_or_else(|| fail("the OS's tree is not in the store".into()))?;
    let os_file = root.join(&to);
    let os_has = std::fs::symlink_metadata(&os_file).ok();
    if os_has.as_ref().is_some_and(|meta| !meta.is_file()) {
        return Err(fail(format!(
            "/{to} in the OS's tree is not a file; only a file is assembled"
        )));
    }
    let mut texts: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    if os_has.is_some() {
        let bytes = std::fs::read(&os_file).map_err(|e| fail(format!("/{to}: {e}")))?;
        texts.insert(OS_PART.to_string(), bytes);
    }
    for part in parts.iter() {
        if part.owner == OS_PART {
            return Err(fail(format!(
                "a component's part of /{to} is owned by `{OS_PART}`, the OS's name"
            )));
        }
        let file = part
            .tree
            .object
            .path_in(&store)
            .map(|dir| dir.join(&to))
            .filter(|file| file.is_file())
            .ok_or_else(|| fail(format!("{}'s part has no file /{to}", part.owner)))?;
        let bytes = std::fs::read(&file).map_err(|e| fail(format!("{}: {e}", part.owner)))?;
        if texts.insert(part.owner.clone(), bytes).is_some() {
            return Err(fail(format!("{} gives two parts of /{to}", part.owner)));
        }
    }
    let text = assemble(&to, &order, &texts, rule_key).map_err(fail)?;
    let (mode, uid, gid) = match &os_has {
        Some(meta) => {
            let entry = base.entries.iter().find(|entry| entry.path == to);
            (
                entry.map_or(meta.permissions().mode() & 0o7777, |e| e.mode),
                entry.map_or(0, |e| e.uid),
                entry.map_or(0, |e| e.gid),
            )
        }
        None => (MODE, 0, 0),
    };
    let key = KeyBuilder::new(NAME, 1)
        .config("path", &to)
        .config("mode", format!("{mode:o}"))
        .config("text", &text)
        .finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("tree", Path::new("tree"))],
        |work| {
            let io = |e: std::io::Error| StepError::body(NAME, e.to_string());
            let dst = work.dir().join("tree").join(&to);
            make_dirs(dst.parent().unwrap_or(work.dir())).map_err(io)?;
            std::fs::write(&dst, &text).map_err(io)?;
            set_mode(&dst, mode).map_err(io)
        },
    )?;
    Ok(Layer {
        owner: OWNER.to_string(),
        tree: Tree {
            object: made.outputs["tree"].into(),
            entries: vec![FileEntry {
                path: to.clone(),
                mode,
                uid,
                gid,
            }],
        },
        replaces: if os_has.is_some() { vec![to] } else { vec![] },
    })
}

/// The file's bytes: the parts `texts` holds (by owner), in `order`, every
/// part named once and every name a part, with no two rules of one name.
pub fn assemble(
    to: &str,
    order: &[String],
    texts: &BTreeMap<String, Vec<u8>>,
    rule_key: usize,
) -> Result<Vec<u8>, String> {
    if order.is_empty() {
        return Err(format!("/{to} has no parts in its order"));
    }
    for (n, name) in order.iter().enumerate() {
        if order[..n].contains(name) {
            return Err(format!("`{name}` is named twice in the order of /{to}"));
        }
        if !texts.contains_key(name) {
            return Err(if name == OS_PART {
                format!("the order of /{to} names `{OS_PART}`, and the OS's tree has no /{to}")
            } else {
                format!("the order of /{to} names {name}, which gives no part of it")
            });
        }
    }
    if let Some(owner) = texts.keys().find(|owner| !order.contains(owner)) {
        return Err(if owner == OS_PART {
            format!(
                "the OS's tree has /{to}, and its order does not name `{OS_PART}`: say where its lines go"
            )
        } else {
            format!("{owner} gives a part of /{to}, and its order does not name it")
        });
    }
    let mut out = Vec::new();
    // Each rule's name: its owner and line number in that owner's part.
    let mut rules: BTreeMap<String, (&str, usize)> = BTreeMap::new();
    for owner in order {
        let bytes = &texts[owner];
        let text = std::str::from_utf8(bytes)
            .map_err(|_| format!("{owner}'s part of /{to} is not UTF-8 text"))?;
        for (n, line) in text.lines().enumerate() {
            let rule = line.trim();
            if rule.is_empty() || rule.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = rule.split_whitespace().collect();
            let name = match rule_key {
                0 => fields.join(" "),
                k => fields.get(k - 1).copied().unwrap_or(rule).to_string(),
            };
            if let Some((first, at)) = rules.get(&name) {
                return Err(format!(
                    "/{to}: the rule `{name}` is in {first} (line {at}) and in {owner} (line {})",
                    n + 1
                ));
            }
            rules.insert(name, (owner, n + 1));
        }
        out.extend_from_slice(bytes);
        if !bytes.is_empty() && !bytes.ends_with(b"\n") {
            out.push(b'\n');
        }
    }
    Ok(out)
}
