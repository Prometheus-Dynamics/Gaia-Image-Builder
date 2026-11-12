//! `gaia.tree.services`: enabled units as the links `systemctl enable`
//! makes.

use crate::walk::make_dirs;
use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Layer, Tree};
use std::path::Path;

const NAME: &str = "gaia.tree.services";
/// Where a unit file is looked for, in this order.
const UNIT_DIRS: [&str; 3] = [
    "etc/systemd/system",
    "usr/lib/systemd/system",
    "lib/systemd/system",
];

/// `owner`'s layer of links that enable each of `units`, read from `tree`
/// (the tree that has the unit files). Keyed on the links it makes.
#[node(
    id = "gaia.tree.services",
    inputs(store, owner, tree, units),
    outputs(layer)
)]
pub fn tree_services(
    store: &str,
    owner: &str,
    tree: Tree,
    units: Vec<String>,
    cx: &Ctx,
) -> Result<Layer, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let root = tree
        .object
        .path_in(&store)
        .ok_or_else(|| StepError::body(NAME, "the unit tree is not in the store"))?;
    let mut links = Vec::new();
    for unit in &units {
        if unit.contains('/') || unit.ends_with("@.service") {
            return Err(StepError::body(
                NAME,
                format!("{owner}: {unit} is not a unit that can be enabled by name"),
            ));
        }
        let Some(dir) = UNIT_DIRS
            .iter()
            .find(|dir| root.join(dir).join(unit).is_file())
        else {
            return Err(StepError::body(
                NAME,
                format!("{owner}: no unit file for {unit}"),
            ));
        };
        let text = std::fs::read_to_string(root.join(dir).join(unit))
            .map_err(|e| StepError::body(NAME, format!("{unit}: {e}")))?;
        let install = install_section(&text);
        if install.is_empty() {
            return Err(StepError::body(
                NAME,
                format!("{owner}: {unit} has no WantedBy or RequiredBy to enable it by"),
            ));
        }
        for (kind, target) in install {
            links.push((
                format!("etc/systemd/system/{target}.{kind}/{unit}"),
                format!("/{dir}/{unit}"),
            ));
        }
    }
    links.sort();
    links.dedup();
    let key = KeyBuilder::new(NAME, 1)
        .config("owner", owner)
        .config(
            "links",
            links
                .iter()
                .map(|(at, to)| format!("{at} {to}"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("tree", Path::new("tree"))],
        |work| {
            let out = work.dir().join("tree");
            make_dirs(&out).map_err(|e| StepError::body(NAME, e))?;
            for (at, to) in &links {
                let link = out.join(at);
                make_dirs(link.parent().unwrap_or(&out)).map_err(|e| StepError::body(NAME, e))?;
                std::os::unix::fs::symlink(to, &link).map_err(|e| StepError::body(NAME, e))?;
            }
            Ok(())
        },
    )?;
    Ok(Layer {
        owner: owner.to_string(),
        tree: Tree {
            object: made.outputs["tree"].into(),
            entries: Vec::new(),
        },
        replaces: Vec::new(),
    })
}

/// `(wants|requires, target)` for each target in the unit's `[Install]`
/// `WantedBy=` and `RequiredBy=`.
fn install_section(text: &str) -> Vec<(&'static str, String)> {
    let mut in_install = false;
    let mut found = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_install = line == "[Install]";
            continue;
        }
        if !in_install {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let kind = match name.trim() {
            "WantedBy" => "wants",
            "RequiredBy" => "requires",
            _ => continue,
        };
        found.extend(value.split_whitespace().map(|t| (kind, t.to_string())));
    }
    found
}
