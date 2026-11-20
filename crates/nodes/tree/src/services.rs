//! `gaia.tree.services`: the units components enable, as the links
//! `systemctl enable` makes.
//!
//! A unit is looked for as systemd does, in `/etc/systemd/system`, then
//! `/usr/lib/systemd/system`, then `/lib/systemd/system`, in the tree the
//! layers merge into (a later layer's path over an earlier one's). Its
//! `[Install]` section, in the unit file itself, gives the links:
//! `WantedBy=`, `RequiredBy=` and `UpheldBy=` a link in the target's
//! `.wants`, `.requires` or `.upholds` directory, `Alias=` a link of that
//! name, and `Also=` another unit enabled the same way. An instance
//! (`getty@tty1.service`) is enabled from its template (`getty@.service`)
//! under its own name. A link the tree already has to the same unit (the
//! OS's presets enabled it) is left as it is.
//!
//! Refused, naming the unit and the component that enables it: a unit with
//! no file, a masked one (a link to `/dev/null`), one with no `[Install]`
//! entry to enable it by, a specifier (`%`) in that section, and a link
//! the tree already has to something else.
//!
//! It also warns, never failing, when a component ships a `sysusers.d` or
//! `system-preset` file that names a user, group, membership or enabled
//! unit no component states in its `component.toml`: nothing applies those
//! files to a read-only root, so what they say is only there if a
//! declaration says it too.

use crate::walk::make_dirs;
use daedalus::prelude::*;
use daedalus_cpu_author::FanIn;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Account, AccountGroup, Layer, Tree};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

const NAME: &str = "gaia.tree.services";
/// Where a unit file is looked for, in this order.
const UNIT_DIRS: [&str; 3] = [
    "etc/systemd/system",
    "usr/lib/systemd/system",
    "lib/systemd/system",
];
/// Links followed in one lookup before it is called a loop.
const HOPS: u32 = 32;

/// The layer `services`: the links that enable `units`, a JSON list of
/// `[component, unit]` pairs, read from the tree `layers` merge into.
/// Keyed on the links it makes. `users` and `groups` (JSON, as
/// `gaia.tree.accounts` takes them) are what [`undeclared`] checks the
/// components' `sysusers.d` files against.
#[node(
    id = "gaia.tree.services",
    inputs(store, layers, units, users, groups),
    outputs(layer)
)]
pub fn tree_services(
    store: &str,
    layers: FanIn<'_, Layer>,
    units: &str,
    users: &str,
    groups: &str,
    cx: &Ctx,
) -> Result<Layer, StepError> {
    let bad = |what: &str, e: serde_json::Error| StepError::body(NAME, format!("the {what}: {e}"));
    let units: Vec<(String, String)> =
        serde_json::from_str(units).map_err(|e| bad("units are not [component, unit] pairs", e))?;
    let users: Vec<Account> =
        serde_json::from_str(users).map_err(|e| bad("users are not a list of users", e))?;
    let groups: Vec<AccountGroup> =
        serde_json::from_str(groups).map_err(|e| bad("groups are not a list of groups", e))?;
    let layers: Vec<Layer> = layers.iter().map(|layer| (*layer).clone()).collect();
    for gap in undeclared(store, &layers, &units, &users, &groups)? {
        eprintln!("warning: {gap}");
    }
    enable_units(store, &layers, &units, cx)
}

/// Where `sysusers.d` files are read from.
const SYSUSERS: [&str; 3] = ["etc/sysusers.d", "usr/lib/sysusers.d", "lib/sysusers.d"];
/// Where preset files are read from.
const PRESETS: [&str; 3] = [
    "etc/systemd/system-preset",
    "usr/lib/systemd/system-preset",
    "lib/systemd/system-preset",
];

/// What the components' own (not the OS's) `sysusers.d` and preset files
/// say that no declaration does: each a line naming the component, the
/// file and the gap.
pub fn undeclared(
    store: &str,
    layers: &[Layer],
    units: &[(String, String)],
    users: &[Account],
    groups: &[AccountGroup],
) -> Result<Vec<String>, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let enabled: BTreeSet<&str> = units.iter().map(|(_, unit)| unit.as_str()).collect();
    let mut group_names: BTreeSet<&str> = groups.iter().map(|g| g.name.as_str()).collect();
    for user in users {
        group_names.insert(user.group.as_deref().unwrap_or(&user.name));
    }
    let user = |name: &str| users.iter().find(|u| u.name == name);
    let mut gaps = Vec::new();
    for layer in layers.iter().filter(|layer| layer.owner != "os") {
        let Some(root) = layer.tree.object.path_in(&store) else {
            continue;
        };
        for (dir, text, file) in files(&root, &SYSUSERS, ".conf") {
            let at = |what: String| format!("{}: /{dir}/{file} {what}", layer.owner);
            for line in text.lines().map(str::trim) {
                let words: Vec<&str> = line.split_whitespace().collect();
                match words.as_slice() {
                    [kind, name, rest @ ..] if kind.trim_end_matches('!') == "u" => {
                        match user(name) {
                            None => gaps.push(at(format!(
                                "has user {name}, which no component's [[users]] states"
                            ))),
                            Some(declared) => {
                                let id = rest.first().and_then(|id| id.split(':').next());
                                if let (Some(want), Some(id)) =
                                    (declared.uid, id.and_then(|id| id.parse::<u32>().ok()))
                                    && want != id
                                {
                                    gaps.push(at(format!(
                                        "gives user {name} uid {id}, and {}'s [[users]] {want}",
                                        declared.owner
                                    )));
                                }
                            }
                        }
                    }
                    ["g", name, ..] if !group_names.contains(name) => gaps.push(at(format!(
                        "has group {name}, which no component's [[groups]] or [[users]] states"
                    ))),
                    ["m", name, group, ..] => {
                        let member = user(name).is_some_and(|u| {
                            u.groups.iter().any(|g| g == group)
                                || u.group.as_deref().unwrap_or(&u.name) == *group
                        });
                        if !member {
                            gaps.push(at(format!(
                                "puts {name} in group {group}, which no component's [[users]] does"
                            )));
                        }
                    }
                    _ => {}
                }
            }
        }
        for (dir, text, file) in files(&root, &PRESETS, ".preset") {
            for line in text.lines().map(str::trim) {
                let words: Vec<&str> = line.split_whitespace().collect();
                if let ["enable", unit, ..] = words.as_slice()
                    && !unit.contains(['*', '?', '['])
                    && !enabled.contains(unit)
                {
                    gaps.push(format!(
                        "{}: /{dir}/{file} enables {unit}, which no component's [[services]] enables",
                        layer.owner
                    ));
                }
            }
        }
    }
    Ok(gaps)
}

/// The files ending in `suffix` in each of `dirs` of one layer's tree,
/// with their text.
fn files(root: &Path, dirs: &[&'static str], suffix: &str) -> Vec<(&'static str, String, String)> {
    let mut found = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(suffix))
            .collect();
        names.sort();
        for name in names {
            if let Ok(text) = std::fs::read_to_string(root.join(dir).join(&name)) {
                found.push((*dir, text, name));
            }
        }
    }
    found
}

/// [`tree_services`] with the layers as a slice.
pub fn enable_units(
    store: &str,
    layers: &[Layer],
    units: &[(String, String)],
    cx: &Ctx,
) -> Result<Layer, StepError> {
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let roots = layers
        .iter()
        .map(|layer| {
            layer.tree.object.path_in(&store).ok_or_else(|| {
                StepError::body(NAME, format!("{}'s tree is not in the store", layer.owner))
            })
        })
        .collect::<Result<Vec<PathBuf>, StepError>>()?;
    let view = View { roots };
    let links = plan(&view, units).map_err(|e| StepError::body(NAME, e))?;
    let key = KeyBuilder::new(NAME, 2)
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
        owner: "services".to_string(),
        tree: Tree {
            object: made.outputs["tree"].into(),
            entries: Vec::new(),
        },
        replaces: Vec::new(),
    })
}

/// The links to make, path in the tree to target, without those the tree
/// already has.
fn plan(view: &View, units: &[(String, String)]) -> Result<BTreeMap<String, String>, String> {
    let mut links: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<(String, String)> = units.iter().cloned().collect();
    while let Some((owner, unit)) = queue.pop_front() {
        if !done.insert(unit.clone()) {
            continue;
        }
        let at = |why: String| format!("{owner}: {unit} {why}");
        let (template, instance) = names(&unit).map_err(at)?;
        let mut found = None;
        'dirs: for dir in UNIT_DIRS {
            for name in instance.iter().chain(std::iter::once(&template)) {
                if let Some(text) = view.unit(&format!("{dir}/{name}")).map_err(at)? {
                    found = Some((format!("/{dir}/{name}"), text));
                    break 'dirs;
                }
            }
        }
        let Some((target, text)) = found else {
            return Err(at("has no unit file in the tree".into()));
        };
        let install = install_section(&text).map_err(at)?;
        if install.is_empty() {
            return Err(at(
                "has no [Install] entry to enable it by (WantedBy, RequiredBy, UpheldBy, Alias or Also)"
                    .into(),
            ));
        }
        let mut add = |path: String| -> Result<(), String> {
            match links.get(&path) {
                Some((to, by)) if *to != target => Err(at(format!(
                    "links /{path} to {target}, which {by} links to {to}"
                ))),
                Some(_) => Ok(()),
                None => {
                    links.insert(path, (target.clone(), unit.clone()));
                    Ok(())
                }
            }
        };
        for entry in install {
            match entry {
                Install::Dir(kind, by) => {
                    add(format!("etc/systemd/system/{by}.{kind}/{unit}"))?;
                }
                Install::Alias(alias) => {
                    if instance.is_some() {
                        return Err(at(format!("is an instance, and has Alias={alias}")));
                    }
                    names(&alias).map_err(at)?;
                    add(format!("etc/systemd/system/{alias}"))?;
                }
                Install::Also(other) => queue.push_back((owner.clone(), other)),
            }
        }
    }
    let mut out = BTreeMap::new();
    for (path, (to, unit)) in links {
        match view.at(&view.real(&path)?) {
            None => {
                out.insert(path, to);
            }
            Some(At::Link(have)) if have == Path::new(&to) => {}
            Some(At::Link(have)) => {
                return Err(format!(
                    "{unit}: /{path} is already in the tree, a link to {}",
                    have.display()
                ));
            }
            Some(_) => {
                return Err(format!(
                    "{unit}: /{path} is already in the tree, not a link"
                ));
            }
        }
    }
    Ok(out)
}

/// A unit's template and, for an instance, its own name.
fn names(unit: &str) -> Result<(String, Option<String>), String> {
    let plain = !unit.is_empty()
        && !unit.contains('/')
        && !unit.starts_with('.')
        && unit
            .rsplit_once('.')
            .is_some_and(|(stem, _)| !stem.is_empty());
    if !plain {
        return Err("is not a unit name".into());
    }
    let Some((prefix, rest)) = unit.split_once('@') else {
        return Ok((unit.to_string(), None));
    };
    let Some((instance, suffix)) = rest.rsplit_once('.') else {
        return Err("is not a unit name".into());
    };
    if instance.is_empty() {
        return Err("is a template: enable an instance of it".into());
    }
    Ok((format!("{prefix}@.{suffix}"), Some(unit.to_string())))
}

/// One entry of an `[Install]` section.
enum Install {
    /// `wants`, `requires` or `upholds`, and the unit it is in.
    Dir(&'static str, String),
    Alias(String),
    Also(String),
}

/// The `[Install]` section's entries that make links.
fn install_section(text: &str) -> Result<Vec<Install>, String> {
    let mut in_install = false;
    let mut found = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_install = line == "[Install]";
            continue;
        }
        if !in_install || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let make: fn(String) -> Install = match name {
            "WantedBy" => |v| Install::Dir("wants", v),
            "RequiredBy" => |v| Install::Dir("requires", v),
            "UpheldBy" => |v| Install::Dir("upholds", v),
            "Alias" => Install::Alias,
            "Also" => Install::Also,
            _ => continue,
        };
        for word in value.split_whitespace() {
            if word.contains('%') {
                return Err(format!("has {name}={word}, a specifier, in [Install]"));
            }
            found.push(make(word.to_string()));
        }
    }
    Ok(found)
}

/// What is at a path of the tree.
enum At {
    File(PathBuf),
    Link(PathBuf),
    Other,
}

/// The tree the layers merge into, read without copying it.
struct View {
    /// Each layer's stored tree, in merge order.
    roots: Vec<PathBuf>,
}

impl View {
    /// What is at `path` (no link in its parents), in the last layer that
    /// has it.
    fn at(&self, path: &str) -> Option<At> {
        self.roots.iter().rev().find_map(|root| {
            let full = root.join(path);
            let meta = std::fs::symlink_metadata(&full).ok()?;
            Some(if meta.file_type().is_symlink() {
                At::Link(std::fs::read_link(&full).ok()?)
            } else if meta.is_file() {
                At::File(full)
            } else {
                At::Other
            })
        })
    }

    /// `path` with the links in its parents followed inside the tree: an
    /// absolute target from the tree's root, never the host's.
    fn real(&self, path: &str) -> Result<String, String> {
        let mut parts: VecDeque<String> = split(path);
        let mut done: Vec<String> = Vec::new();
        let mut hops = 0;
        while let Some(part) = parts.pop_front() {
            if part == ".." {
                done.pop();
                continue;
            }
            done.push(part);
            if parts.is_empty() {
                break;
            }
            if let Some(At::Link(to)) = self.at(&done.join("/")) {
                hops += 1;
                if hops > HOPS {
                    return Err(format!("/{path}: too many links"));
                }
                done.pop();
                let to = to.to_string_lossy().into_owned();
                if to.starts_with('/') {
                    done.clear();
                }
                for (i, part) in split(&to).into_iter().enumerate() {
                    parts.insert(i, part);
                }
            }
        }
        Ok(done.join("/"))
    }

    /// The text of the unit file at `path`, following links to it inside
    /// the tree; none when there is nothing there; an error when it is
    /// masked.
    fn unit(&self, path: &str) -> Result<Option<String>, String> {
        let mut path = self.real(path)?;
        for _ in 0..HOPS {
            match self.at(&path) {
                None => return Ok(None),
                Some(At::File(file)) => {
                    return std::fs::read_to_string(&file)
                        .map(Some)
                        .map_err(|e| format!("/{path}: {e}"));
                }
                Some(At::Other) => return Err(format!("/{path} is not a file")),
                Some(At::Link(to)) if to == Path::new("/dev/null") => {
                    return Err(format!("is masked: /{path} is a link to /dev/null"));
                }
                Some(At::Link(to)) => {
                    let to = to.to_string_lossy().into_owned();
                    let next = if to.starts_with('/') {
                        to
                    } else {
                        let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
                        format!("{parent}/{to}")
                    };
                    path = self.real(&next)?;
                }
            }
        }
        Err(format!("/{path}: too many links"))
    }
}

/// A path's names, without empty ones and `.`.
fn split(path: &str) -> VecDeque<String> {
    path.split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(str::to_string)
        .collect()
}
