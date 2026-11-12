//! The delta of a report against the one before it.

use crate::model::{Delta, FileChange, OwnerChange, Report, SourceChange};
use std::collections::{BTreeMap, BTreeSet};

/// `after` minus `before`, saturating at the `i64` range.
fn diff(after: u64, before: u64) -> i64 {
    let change = i128::from(after) - i128::from(before);
    i64::try_from(change).unwrap_or(if change < 0 { i64::MIN } else { i64::MAX })
}

/// What changed from `previous` to `current`: image and total bytes, each
/// owner's bytes, each locked source's resolved commit or digest, and each
/// file that was added, removed or changed in bytes, mode or owners.
pub(crate) fn compare(previous: &Report, current: &Report) -> Delta {
    let owners: BTreeSet<&str> = previous
        .components
        .iter()
        .chain(&current.components)
        .map(|c| c.owner.as_str())
        .collect();
    let components = owners
        .iter()
        .filter_map(|owner| {
            let bytes = |report: &Report| {
                report
                    .components
                    .iter()
                    .find(|c| c.owner == *owner)
                    .map_or(0, |c| c.bytes)
            };
            let (before, after) = (bytes(previous), bytes(current));
            (before != after).then(|| OwnerChange {
                owner: owner.to_string(),
                before,
                after,
            })
        })
        .collect();

    let sources = |report: &Report| -> BTreeMap<String, String> {
        report
            .sources
            .iter()
            .map(|s| (format!("{}.{}", s.owner, s.source), s.resolved.clone()))
            .collect()
    };
    let (old, new) = (sources(previous), sources(current));
    let names: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    let sources = names
        .into_iter()
        .filter(|name| old.get(*name) != new.get(*name))
        .map(|name| SourceChange {
            name: name.clone(),
            before: old.get(name).cloned(),
            after: new.get(name).cloned(),
        })
        .collect();

    let before: BTreeMap<&str, _> = previous
        .files
        .iter()
        .map(|f| (f.path.as_str(), f))
        .collect();
    let after: BTreeMap<&str, _> = current.files.iter().map(|f| (f.path.as_str(), f)).collect();
    let added = after
        .keys()
        .filter(|path| !before.contains_key(*path))
        .map(|path| path.to_string())
        .collect();
    let removed = before
        .keys()
        .filter(|path| !after.contains_key(*path))
        .map(|path| path.to_string())
        .collect();
    let changed = after
        .iter()
        .filter_map(|(path, now)| {
            let was = before.get(path)?;
            (was.bytes != now.bytes || was.mode != now.mode || was.owners != now.owners).then(
                || FileChange {
                    path: path.to_string(),
                    before_bytes: was.bytes,
                    after_bytes: now.bytes,
                    before_mode: was.mode.clone(),
                    after_mode: now.mode.clone(),
                    before_owners: was.owners.clone(),
                    after_owners: now.owners.clone(),
                },
            )
        })
        .collect();

    Delta {
        image_change: diff(current.image_bytes, previous.image_bytes),
        total_change: diff(current.total_bytes, previous.total_bytes),
        components,
        sources,
        added,
        removed,
        changed,
    }
}
