//! `gaia.tree.accounts`: the users and groups components need, added to the
//! base's account files.

use crate::walk::make_dirs;
use daedalus::prelude::*;
use gaia_step::{Ctx, KeyBuilder, Step, StepError, Store};
use gaia_types::{Account, AccountGroup, FileEntry, Layer, Tree};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const NAME: &str = "gaia.tree.accounts";
/// Ids not given are the lowest free in this range: Buildroot's mkusers
/// range for system users, which every user a component states is (a
/// daemon's, with no login). The base's ids are taken first, so an id is
/// the same on every build of the same base and set of declarations.
const FIRST_ID: u32 = 100;
const LAST_ID: u32 = 999;
const FILES: [(&str, u32); 3] = [
    ("etc/passwd", 0o644),
    ("etc/group", 0o644),
    ("etc/shadow", 0o600),
];

/// Keyed on the three files it writes, so a change elsewhere in the base
/// does not remake it.
///
/// The layer `accounts` that replaces the base's `/etc/passwd`,
/// `/etc/group` and `/etc/shadow` with them plus `users` and `groups`
/// (JSON lists of [`Account`] and [`AccountGroup`]).
#[node(
    id = "gaia.tree.accounts",
    inputs(store, base, users, groups),
    outputs(layer)
)]
pub fn tree_accounts(
    store: &str,
    base: Tree,
    users: &str,
    groups: &str,
    cx: &Ctx,
) -> Result<Layer, StepError> {
    let users: Vec<Account> = serde_json::from_str(users)
        .map_err(|e| StepError::body(NAME, format!("the users are not a list of users: {e}")))?;
    let groups: Vec<AccountGroup> = serde_json::from_str(groups)
        .map_err(|e| StepError::body(NAME, format!("the groups are not a list of groups: {e}")))?;
    let store = Store::open(store).map_err(StepError::store(NAME))?;
    let root = base
        .object
        .path_in(&store)
        .ok_or_else(|| StepError::body(NAME, "the base tree is not in the store"))?;
    if root.join("etc/gshadow").exists() {
        return Err(StepError::body(
            NAME,
            "the base has /etc/gshadow, which components' groups are not added to",
        ));
    }
    let read = |name: &str| std::fs::read_to_string(root.join(name)).unwrap_or_default();
    let (passwd, group, shadow) = (read("etc/passwd"), read("etc/group"), read("etc/shadow"));
    let files =
        plan(&passwd, &group, &shadow, &users, &groups).map_err(|e| StepError::body(NAME, e))?;
    let key = KeyBuilder::new(NAME, 1)
        .config("passwd", &files[0])
        .config("group", &files[1])
        .config("shadow", &files[2])
        .finish();
    let made = Step::new(&store, cx, NAME, key).look_up_or_make(
        &[("tree", Path::new("tree"))],
        |work| {
            let etc = work.dir().join("tree/etc");
            make_dirs(&etc).map_err(|e| StepError::body(NAME, e))?;
            for ((name, mode), text) in FILES.iter().zip(&files) {
                let path = work.dir().join("tree").join(name);
                std::fs::write(&path, text).map_err(|e| StepError::body(NAME, e))?;
                crate::walk::set_mode(&path, *mode).map_err(|e| StepError::body(NAME, e))?;
            }
            Ok(())
        },
    )?;
    Ok(Layer {
        owner: "accounts".to_string(),
        tree: Tree {
            object: made.outputs["tree"].into(),
            entries: FILES
                .iter()
                .map(|(path, mode)| FileEntry {
                    path: path.to_string(),
                    mode: *mode,
                    uid: 0,
                    gid: 0,
                })
                .collect(),
        },
        replaces: FILES.iter().map(|(path, _)| path.to_string()).collect(),
    })
}

/// A line of `/etc/group`.
struct GroupLine {
    name: String,
    gid: u32,
    members: BTreeSet<String>,
    /// From the base: written back as it was unless a member is added.
    text: Option<String>,
}

/// The new `passwd`, `group` and `shadow`.
fn plan(
    passwd: &str,
    group: &str,
    shadow: &str,
    users: &[Account],
    groups: &[AccountGroup],
) -> Result<[String; 3], String> {
    // The base's.
    let mut base_users: BTreeMap<String, u32> = BTreeMap::new();
    for line in passwd.lines().filter(|l| !l.trim().is_empty()) {
        let fields: Vec<&str> = line.split(':').collect();
        let uid = fields.get(2).and_then(|u| u.parse().ok());
        base_users.insert(fields[0].to_string(), uid.unwrap_or(u32::MAX));
    }
    let base_uids: BTreeMap<u32, &str> = base_users
        .iter()
        .map(|(name, uid)| (*uid, name.as_str()))
        .collect();
    let mut lines: Vec<GroupLine> = Vec::new();
    for line in group.lines().filter(|l| !l.trim().is_empty()) {
        let fields: Vec<&str> = line.split(':').collect();
        lines.push(GroupLine {
            name: fields[0].to_string(),
            gid: fields
                .get(2)
                .and_then(|g| g.parse().ok())
                .unwrap_or(u32::MAX),
            members: fields
                .get(3)
                .map(|m| {
                    m.split(',')
                        .filter(|m| !m.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            text: Some(line.to_string()),
        });
    }
    let base_groups = lines.len();

    // One declaration per name; the same one from two components is fine.
    let mut wanted_groups: BTreeMap<&str, &AccountGroup> = BTreeMap::new();
    for g in groups {
        if let Some(first) = wanted_groups.insert(&g.name, g)
            && first.gid != g.gid
        {
            return Err(format!(
                "group {}: gid {:?} in {}, {:?} in {}",
                g.name, first.gid, first.owner, g.gid, g.owner
            ));
        }
    }
    let mut wanted_users: BTreeMap<&str, &Account> = BTreeMap::new();
    for u in users {
        if let Some(first) = wanted_users.insert(&u.name, u)
            && (
                first.uid,
                &first.group,
                &first.groups,
                &first.home,
                &first.shell,
            ) != (u.uid, &u.group, &u.groups, &u.home, &u.shell)
        {
            return Err(format!(
                "user {}: declared differently by {} and {}",
                u.name, first.owner, u.owner
            ));
        }
    }

    // A fixed id is its name's alone: not the base's for another name,
    // not another declaration's.
    let mut fixed_gids: BTreeMap<u32, &AccountGroup> = BTreeMap::new();
    for g in wanted_groups.values() {
        let Some(gid) = g.gid else { continue };
        if let Some(line) = lines.iter().find(|l| l.gid == gid && l.name != g.name) {
            return Err(format!(
                "group {} in {}: gid {gid} is the base's group {}",
                g.name, g.owner, line.name
            ));
        }
        if let Some(other) = fixed_gids.insert(gid, g) {
            return Err(format!(
                "gid {gid}: group {} in {} and group {} in {}",
                other.name, other.owner, g.name, g.owner
            ));
        }
    }
    let mut fixed_uids: BTreeMap<u32, &Account> = BTreeMap::new();
    for u in wanted_users.values() {
        let Some(uid) = u.uid else { continue };
        if let Some(other) = base_uids.get(&uid).filter(|other| **other != u.name) {
            return Err(format!(
                "user {} in {}: uid {uid} is the base's user {other}",
                u.name, u.owner
            ));
        }
        if let Some(other) = fixed_uids.insert(uid, u) {
            return Err(format!(
                "uid {uid}: user {} in {} and user {} in {}",
                other.name, other.owner, u.name, u.owner
            ));
        }
    }

    // Ids in use, then each new group and user, in name order.
    let mut gids: BTreeSet<u32> = lines.iter().map(|l| l.gid).collect();
    let mut uids: BTreeSet<u32> = base_users.values().copied().collect();
    uids.extend(wanted_users.values().filter_map(|u| u.uid));
    gids.extend(wanted_groups.values().filter_map(|g| g.gid));
    let mut add_group =
        |lines: &mut Vec<GroupLine>, name: &str, gid: Option<u32>, own: Option<u32>| {
            if let Some(line) = lines.iter().find(|l| l.name == name) {
                return match gid {
                    Some(gid) if gid != line.gid => Err(format!(
                        "group {name}: gid {gid}, but the base has it as {}",
                        line.gid
                    )),
                    _ => Ok(()),
                };
            }
            let gid = match gid.or(own.filter(|id| !gids.contains(id))) {
                Some(gid) => gid,
                None => free(&gids).ok_or_else(|| format!("group {name}: {FULL}"))?,
            };
            gids.insert(gid);
            lines.push(GroupLine {
                name: name.to_string(),
                gid,
                members: BTreeSet::new(),
                text: None,
            });
            Ok(())
        };
    for g in wanted_groups.values() {
        add_group(&mut lines, &g.name, g.gid, None)?;
    }
    let mut new_users = Vec::new();
    for u in wanted_users.values() {
        if let Some(&uid) = base_users.get(&u.name) {
            if u.uid.is_some_and(|want| want != uid) {
                return Err(format!(
                    "user {} in {}: uid {:?}, but the base has it as {uid}",
                    u.name, u.owner, u.uid
                ));
            }
        } else {
            let uid = match u.uid {
                Some(uid) => uid,
                None => free(&uids).ok_or_else(|| format!("user {}: {FULL}", u.name))?,
            };
            uids.insert(uid);
            new_users.push((*u, uid));
        }
    }
    let mut passwd_out: String = passwd
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| format!("{l}\n"))
        .collect();
    let mut shadow_out: String = shadow
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| format!("{l}\n"))
        .collect();
    for (u, uid) in &new_users {
        let primary = u.group.clone().unwrap_or_else(|| u.name.clone());
        if u.group.is_none() {
            add_group(&mut lines, &primary, None, Some(*uid))?;
        }
        let gid = lines
            .iter()
            .find(|l| l.name == primary)
            .map(|l| l.gid)
            .ok_or_else(|| format!("user {} in {}: no group {primary}", u.name, u.owner))?;
        let home = u.home.as_deref().unwrap_or("/");
        let shell = u.shell.as_deref().unwrap_or("/bin/false");
        passwd_out.push_str(&format!("{}:x:{uid}:{gid}::{home}:{shell}\n", u.name));
        // As mkusers writes a package's system user: no password can match.
        shadow_out.push_str(&format!("{}:*:::::::\n", u.name));
    }
    for u in wanted_users.values() {
        for name in &u.groups {
            let line = lines
                .iter_mut()
                .find(|l| &l.name == name)
                .ok_or_else(|| format!("user {} in {}: no group {name}", u.name, u.owner))?;
            if line.members.insert(u.name.clone()) {
                line.text = None;
            }
        }
    }
    // Base groups in their order, then new ones by gid.
    lines[base_groups..].sort_by_key(|l| l.gid);
    let group_out: String = lines
        .iter()
        .map(|l| match &l.text {
            Some(text) => format!("{text}\n"),
            None => {
                let members: Vec<&str> = l.members.iter().map(String::as_str).collect();
                format!("{}:x:{}:{}\n", l.name, l.gid, members.join(","))
            }
        })
        .collect();
    Ok([passwd_out, group_out, shadow_out])
}

/// Why an id could not be given.
const FULL: &str = "no id from 100 to 999 is free";

/// The lowest id from [`FIRST_ID`] to [`LAST_ID`] not in `used`.
fn free(used: &BTreeSet<u32>) -> Option<u32> {
    (FIRST_ID..=LAST_ID).find(|id| !used.contains(id))
}
