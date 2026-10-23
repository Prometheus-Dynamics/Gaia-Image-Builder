//! Which layer each rootfs path belongs to: the selectors' members, their ranks and the choice.

use super::*;

/// Reads the packages, stage ownership, installs and services that the
/// selectors name, keyed by rootfs path (no leading slash).
pub(super) fn read_membership(
    spec: &ResolvedBuildSpec,
    tree_dir: &Path,
) -> Result<Membership, ImageProviderError> {
    let mut packages: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    if let Ok(list) = fs::read_to_string(tree_dir.join(PACKAGE_FILE_LIST)) {
        for line in list.lines() {
            let Some((package, path)) = line.split_once(',') else {
                continue;
            };
            packages
                .entry(normalize_path(path))
                .or_default()
                .insert(package.to_string());
        }
    }
    let mut stage: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    if let Some(records) = crate::feed_stage_owned::read_stage_ownership(tree_dir)? {
        for record in records {
            stage
                .entry(normalize_path(&record.rel))
                .or_default()
                .insert(record.item);
        }
    }
    let installs = spec
        .install
        .entries
        .iter()
        .map(|entry| (normalize_path(&entry.dest), entry.id.as_str().to_string()))
        .collect();
    let services = spec
        .stage
        .services
        .iter()
        .map(|service| {
            (
                normalize_path(&service.unit_path),
                service.id.as_str().to_string(),
            )
        })
        .collect();
    Ok(Membership {
        packages,
        stage,
        installs,
        services,
    })
}

/// Which layer (index into `order`) each source entry belongs to, plus the
/// directories every layer needs above its members.
pub(super) fn assign_members(
    layers: &ImageLayersSpec,
    membership: &Membership,
    source: &BTreeMap<String, Entry>,
) -> Result<Vec<LayerMembers>, ImageProviderError> {
    let by_name: BTreeMap<&str, &ImageLayerSpec> = layers
        .layers
        .iter()
        .map(|layer| (layer.name.as_str(), layer))
        .collect();
    let selectors: Vec<Option<&ImageLayerSpec>> = layers
        .order
        .iter()
        .map(|name| by_name.get(name.as_str()).copied())
        .collect();
    let default = layers
        .order
        .iter()
        .position(|name| *name == layers.default)
        .ok_or_else(|| {
            runtime_error(format!(
                "image.layers default '{}' is not in order",
                layers.default
            ))
        })?;
    let overrides: BTreeSet<String> = layers
        .overrides
        .iter()
        .map(|path| normalize_path(path))
        .collect();
    let mut members: Vec<LayerMembers> = vec![BTreeMap::new(); layers.order.len()];
    let mut synthetic: BTreeMap<String, Entry> = BTreeMap::new();
    for (path, entry) in source {
        let is_dir = entry.kind == EntryKind::Dir;
        let chosen = choose_layer(
            path,
            is_dir,
            &selectors,
            membership,
            default,
            &overrides,
            &layers.order,
        )?;
        members[chosen].insert(path.clone(), entry.clone());
    }
    // A layer carries the directories above its members, so it extracts on
    // its own. A directory the source lacks is root-owned, 0755.
    for layer in members.iter_mut() {
        let paths: Vec<String> = layer
            .keys()
            .filter(|path| !path.is_empty())
            .cloned()
            .collect();
        let mut needed = BTreeSet::new();
        for path in paths {
            let mut ancestor = String::new();
            needed.insert(String::new());
            let parts: Vec<&str> = path.split('/').collect();
            for part in &parts[..parts.len() - 1] {
                if !ancestor.is_empty() {
                    ancestor.push('/');
                }
                ancestor.push_str(part);
                needed.insert(ancestor.clone());
            }
        }
        for dir in needed {
            if layer.contains_key(&dir) {
                continue;
            }
            let entry = match source.get(&dir) {
                Some(entry) if entry.kind == EntryKind::Dir => entry.clone(),
                _ => synthetic
                    .entry(dir.clone())
                    .or_insert_with(|| Entry::synthetic_dir(&dir))
                    .clone(),
            };
            layer.insert(dir, entry);
        }
    }
    Ok(members)
}

/// The layer for one path: the highest-ranked selectors that match it, with
/// the default when none does.
pub(super) fn choose_layer(
    path: &str,
    is_dir: bool,
    selectors: &[Option<&ImageLayerSpec>],
    membership: &Membership,
    default: usize,
    overrides: &BTreeSet<String>,
    order: &[String],
) -> Result<usize, ImageProviderError> {
    let mut best: Option<u8> = None;
    let mut claimed: Vec<usize> = Vec::new();
    for (index, selector) in selectors.iter().enumerate() {
        let Some(layer) = selector else {
            continue;
        };
        let rank = rank_of(path, layer, membership);
        let Some(rank) = rank else {
            continue;
        };
        match best {
            Some(top) if rank < top => {}
            Some(top) if rank == top => claimed.push(index),
            _ => {
                best = Some(rank);
                claimed = vec![index];
            }
        }
    }
    if best.is_none() {
        return Ok(default);
    }
    match claimed.as_slice() {
        [only] => Ok(*only),
        many => {
            // Directories repeat, so they take the upper layer without a
            // conflict; a file needs an override to say which copy ships.
            let upper = *many.iter().max().unwrap_or(&default);
            if is_dir || overrides.contains(path) {
                return Ok(upper);
            }
            let names: Vec<&str> = many.iter().map(|index| order[*index].as_str()).collect();
            Err(runtime_error(format!(
                "rootfs path '/{path}' is selected by layers {}: list it in image.layers overrides to ship the upper layer's copy",
                names.join(", ")
            )))
        }
    }
}

/// The rank of a layer's selectors for `path`, when one matches: 4 for
/// `paths`, 3 for stage, install and service ids, 2 for packages.
pub(super) fn rank_of(path: &str, layer: &ImageLayerSpec, membership: &Membership) -> Option<u8> {
    let under = |prefix: &str| {
        prefix.is_empty() || path == prefix || path.starts_with(&format!("{prefix}/"))
    };
    if layer
        .paths
        .iter()
        .any(|prefix| under(&normalize_path(prefix)))
    {
        return Some(4);
    }
    if let Some(ids) = membership.stage.get(path)
        && any_glob(&layer.stage, ids.iter())
    {
        return Some(3);
    }
    if membership
        .installs
        .iter()
        .any(|(dest, id)| under(dest.as_str()) && any_glob(&layer.installs, std::iter::once(id)))
    {
        return Some(3);
    }
    if membership
        .services
        .iter()
        .any(|(unit, id)| unit == path && any_glob(&layer.services, std::iter::once(id)))
    {
        return Some(3);
    }
    if let Some(packages) = membership.packages.get(path)
        && any_glob(&layer.packages, packages.iter())
    {
        return Some(2);
    }
    None
}

/// Whether any id matches any of the globs.
pub(super) fn any_glob<'a>(globs: &[String], ids: impl IntoIterator<Item = &'a String>) -> bool {
    ids.into_iter()
        .any(|id| globs.iter().any(|glob| wildcard_match(glob, id)))
}
