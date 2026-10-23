//! `[image.layers]`: the Buildroot root filesystem split into one tar per
//! layer (`base`, `platform`, `app`, ...), each with an optional erofs image
//! and dm-verity tree, and `layers.json` describing them.
//!
//! Buildroot's `rootfs.tar` is the source: it already carries the ownership
//! Buildroot applied under fakeroot, so each layer keeps its entries' uid,
//! gid and mode. Every rootfs path lands in exactly one layer; directories
//! repeat into each layer that has members below them.
//!
//! Membership, by the highest-ranked selector that matches a path:
//! `paths` (rootfs prefixes) > `stage` / `installs` / `services` (the ids
//! that wrote the path) > `packages` (the Buildroot package that installed
//! it, from `build/packages-file-list.txt`) > `default`. Two layers with
//! selectors of the same rank claiming one non-directory path is an error,
//! unless the path is in `overrides`, where the upper layer's copy ships.
//!
//! Each layer is keyed by a fingerprint of its members (path, kind, mode,
//! owner, size and content digest or link target), so a layer whose members
//! did not change is reused as it is: no tar, erofs or verity work.
use super::layers_erofs::{VerityRecord, run_mkfs_erofs, uuid_from_hex, verity_format};
use super::layers_tar::{
    Entry, EntryKind, LayerTarWriter, copy_data, normalize_path, read_entries,
};
use super::*;
use gaia_spec::{ImageLayerSpec, ImageLayersSpec, wildcard_match};
use sha2::{Digest, Sha256};
use std::io::{BufWriter, Write};

/// The directory, under the image's collect dir, that holds the layers.
pub(crate) const LAYERS_DIR: &str = "layers";
/// The description of the layers, beside their outputs.
pub(crate) const LAYERS_JSON: &str = "layers.json";
/// Version of `layers.json`.
const LAYERS_SCHEMA: u64 = 1;
/// Buildroot's rootfs tar, under the build's output directory.
const ROOTFS_TAR: &str = "images/rootfs.tar";
/// The file listing each package's installed paths (`package,./path`).
const PACKAGE_FILE_LIST: &str = "build/packages-file-list.txt";
/// The erofs block size and the verity block sizes (both 4 KiB).
pub(crate) const VERITY_BLOCK: u64 = 4096;

/// Where each rootfs path's selectors point, read once per build.
struct Membership {
    /// Path -> the Buildroot packages that installed it.
    packages: BTreeMap<String, BTreeSet<String>>,
    /// Path -> the stage file ids that wrote it.
    stage: BTreeMap<String, BTreeSet<String>>,
    /// Install destinations (normalized) with their ids.
    installs: Vec<(String, String)>,
    /// Service unit paths (normalized) with their stage service ids.
    services: Vec<(String, String)>,
}

/// One layer's emitted members: its entries by path, in path order.
type LayerMembers = BTreeMap<String, Entry>;

/// What a layer writes for an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Emit {
    /// The entry as it is.
    Plain,
    /// A hardlink to a target in the same layer.
    Link(String),
    /// A hardlink whose target is in another layer: a regular copy of the
    /// target's data.
    Copy(String),
}

/// One layer's result, as `layers.json` records it.
struct BuiltLayer {
    json: serde_json::Value,
    line: String,
}

/// Splits the image's rootfs tar into layers under `collect_dir/layers`,
/// reusing every layer whose members did not change. Returns the summary
/// lines.
pub(crate) fn build_layers(
    spec: &ResolvedBuildSpec,
    layers: &ImageLayersSpec,
    tree_dir: &Path,
    collect_dir: &Path,
) -> Result<Vec<String>, ImageProviderError> {
    let rootfs_tar = tree_dir.join(ROOTFS_TAR);
    if !rootfs_tar.is_file() {
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::OutputMissing,
            format!(
                "image.layers needs Buildroot's rootfs tar, which is missing at '{}' (BR2_TARGET_ROOTFS_TAR)",
                rootfs_tar.display()
            ),
        ));
    }
    let source = read_entries(&rootfs_tar).map_err(runtime_error)?;
    let membership = read_membership(spec, tree_dir)?;
    let members = assign_members(layers, &membership, &source)?;
    let mtime = crate::source_date::source_date_epoch(spec)
        .and_then(|epoch| epoch.parse::<u64>().ok())
        .unwrap_or(0);
    let layers_dir = collect_dir.join(LAYERS_DIR);
    fs::create_dir_all(&layers_dir).map_err(|error| {
        runtime_error(format!(
            "failed to create layers dir '{}': {error}",
            layers_dir.display()
        ))
    })?;
    let old = read_previous(&layers_dir)?;
    let mut built = Vec::new();
    let mut messages = Vec::new();
    let mut reused = 0usize;
    for (index, name) in layers.order.iter().enumerate() {
        let plan = plan_layer(&members[index], &source)?;
        let inputs = layer_fingerprint(&plan, &members[index], &source, mtime, layers);
        let previous = old.iter().find(|value| value["name"] == name.as_str());
        let layer =
            match previous.filter(|value| reusable(value, name, &inputs, &layers_dir, layers)) {
                Some(value) => {
                    reused += 1;
                    BuiltLayer {
                        json: value.clone(),
                        line: format!(
                            "layer {name}: {} bytes, {} entries, reused",
                            value["tar_size"],
                            members[index].len()
                        ),
                    }
                }
                None => build_layer(LayerBuild {
                    name,
                    plan: &plan,
                    members: &members[index],
                    source: &source,
                    rootfs_tar: &rootfs_tar,
                    layers_dir: &layers_dir,
                    inputs: &inputs,
                    mtime,
                    layers,
                    host_dir: &tree_dir.join("host"),
                })?,
            };
        built.push(layer.json);
        messages.push(layer.line);
    }
    remove_stale_outputs(&layers_dir, &layers.order)?;
    write_layers_json(&layers_dir, layers, &built)?;
    messages.push(format!(
        "layers: {} of {} reused, {} rebuilt; described by '{}'",
        reused,
        layers.order.len(),
        layers.order.len() - reused,
        layers_dir.join(LAYERS_JSON).display()
    ));
    Ok(messages)
}

pub(crate) fn runtime_error(message: impl Into<String>) -> ImageProviderError {
    ImageProviderError::new(ImageProviderErrorKind::RuntimeState, message)
}

/// Reads the packages, stage ownership, installs and services that the
/// selectors name, keyed by rootfs path (no leading slash).
fn read_membership(
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
fn assign_members(
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
fn choose_layer(
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
fn rank_of(path: &str, layer: &ImageLayerSpec, membership: &Membership) -> Option<u8> {
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
fn any_glob<'a>(globs: &[String], ids: impl IntoIterator<Item = &'a String>) -> bool {
    ids.into_iter()
        .any(|id| globs.iter().any(|glob| wildcard_match(glob, id)))
}

/// The entries a layer writes, in path order. A hardlink whose target is in
/// another layer becomes a copy of the target's data.
fn plan_layer(
    members: &LayerMembers,
    source: &BTreeMap<String, Entry>,
) -> Result<Vec<(String, Emit)>, ImageProviderError> {
    let mut plan = Vec::with_capacity(members.len());
    for (path, entry) in members {
        let emit = if entry.kind == EntryKind::Hardlink {
            let target = &entry.link;
            match source.get(target) {
                Some(target_entry) if target_entry.kind == EntryKind::File => {}
                _ => {
                    return Err(runtime_error(format!(
                        "hardlink '/{path}' points at '/{target}', which is not a regular file in rootfs.tar"
                    )));
                }
            }
            if members.contains_key(target) {
                Emit::Link(target.clone())
            } else {
                Emit::Copy(target.clone())
            }
        } else {
            Emit::Plain
        };
        plan.push((path.clone(), emit));
    }
    Ok(plan)
}

/// The layer's fingerprint: its members' paths, kinds, owners, sizes and
/// content digests or link targets, under the settings that shape its outputs.
fn layer_fingerprint(
    plan: &[(String, Emit)],
    members: &LayerMembers,
    source: &BTreeMap<String, Entry>,
    mtime: u64,
    layers: &ImageLayersSpec,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(
        format!(
            "gaia-layer-v1 mtime={mtime} erofs={} verity={}\n",
            layers.erofs, layers.verity
        )
        .as_bytes(),
    );
    for (path, emit) in plan {
        let entry = &members[path];
        let detail = match (entry.kind, emit) {
            (EntryKind::Dir, _) => "dir".to_string(),
            (EntryKind::File, _) => format!("file:{}", entry.digest),
            (EntryKind::Symlink, _) => format!("link:{}", entry.link),
            (EntryKind::Hardlink, Emit::Link(target)) => format!("hardlink:{target}"),
            (EntryKind::Hardlink, Emit::Copy(target)) => {
                format!("file:{}", source[target].digest)
            }
            (EntryKind::Hardlink, Emit::Plain) => "hardlink".to_string(),
            (EntryKind::CharDev, _) => format!("chr:{},{}", entry.dev_major, entry.dev_minor),
            (EntryKind::BlockDev, _) => format!("blk:{},{}", entry.dev_major, entry.dev_minor),
            (EntryKind::Fifo, _) => "fifo".to_string(),
        };
        let size = match emit {
            Emit::Copy(target) => source[target].size,
            _ => entry.size,
        };
        hasher.update(
            format!(
                "{path}\t{detail}\t{:o}\t{}\t{}\t{size}\n",
                entry.mode, entry.uid, entry.gid
            )
            .as_bytes(),
        );
    }
    format!("sha256:{}", gaia_process::hex(&hasher.finalize()))
}

/// Whether the previous layer record can be reused: same inputs, and its
/// outputs are on disk and still hash to what it recorded.
fn reusable(
    previous: &serde_json::Value,
    name: &str,
    inputs: &str,
    layers_dir: &Path,
    layers: &ImageLayersSpec,
) -> bool {
    if previous["name"] != name || previous["inputs"] != inputs {
        return false;
    }
    let tar = layers_dir.join(format!("{name}.tar"));
    if previous["tar"] != format!("{name}.tar")
        || !tar_matches(
            &tar,
            previous["tar_size"].as_u64(),
            previous["tar_sha256"].as_str(),
        )
    {
        return false;
    }
    if !layers.erofs {
        return previous["erofs"].is_null();
    }
    let erofs = layers_dir.join(format!("{name}.erofs"));
    tar_matches(
        &erofs,
        previous["erofs_size"].as_u64(),
        previous["erofs_sha256"].as_str(),
    )
}

fn tar_matches(path: &Path, size: Option<u64>, sha256: Option<&str>) -> bool {
    let (Some(size), Some(sha256)) = (size, sha256) else {
        return false;
    };
    fs::metadata(path).is_ok_and(|meta| meta.len() == size)
        && gaia_process::sha256_file(path).is_ok_and(|digest| digest == sha256)
}

/// Everything one layer build needs.
struct LayerBuild<'a> {
    name: &'a str,
    plan: &'a [(String, Emit)],
    members: &'a LayerMembers,
    source: &'a BTreeMap<String, Entry>,
    rootfs_tar: &'a Path,
    layers_dir: &'a Path,
    inputs: &'a str,
    mtime: u64,
    layers: &'a ImageLayersSpec,
    /// Buildroot's host dir: its mkfs.erofs and veritysetup are used first.
    host_dir: &'a Path,
}

/// Writes one layer's tar, erofs and verity, and describes them.
fn build_layer(request: LayerBuild<'_>) -> Result<BuiltLayer, ImageProviderError> {
    let LayerBuild {
        name,
        plan,
        members,
        source,
        rootfs_tar,
        layers_dir,
        inputs,
        mtime,
        layers,
        host_dir,
    } = request;
    let tar_path = layers_dir.join(format!("{name}.tar"));
    let tar_temp = layers_dir.join(format!(".{name}.tar.tmp"));
    write_layer_tar(&tar_temp, plan, members, source, rootfs_tar, mtime)?;
    publish(&tar_temp, &tar_path)?;
    let tar_sha256 = gaia_process::sha256_file(&tar_path).map_err(|error| {
        runtime_error(format!("failed to hash '{}': {error}", tar_path.display()))
    })?;
    let tar_size = file_len(&tar_path)?;
    let mut line = format!("layer {name}: {tar_size} bytes tar, {} entries", plan.len());
    let mut erofs = None;
    if layers.erofs {
        let erofs_path = layers_dir.join(format!("{name}.erofs"));
        let erofs_temp = layers_dir.join(format!(".{name}.erofs.tmp"));
        run_mkfs_erofs(
            host_dir,
            &erofs_temp,
            &tar_path,
            &uuid_from_hex(&tar_sha256),
        )?;
        // The verity tree is appended before the image is hashed: the id is
        // the sha256 of the file as shipped.
        let verity = if layers.verity {
            Some(verity_format(host_dir, &erofs_temp, name)?)
        } else {
            None
        };
        publish(&erofs_temp, &erofs_path)?;
        let erofs_size = file_len(&erofs_path)?;
        let erofs_sha256 = gaia_process::sha256_file(&erofs_path).map_err(|error| {
            runtime_error(format!(
                "failed to hash '{}': {error}",
                erofs_path.display()
            ))
        })?;
        line.push_str(&format!(", erofs {erofs_size} bytes"));
        erofs = Some((erofs_sha256, erofs_size, verity));
    }
    line.push_str(", rebuilt");
    let json = layer_json(name, &tar_sha256, tar_size, erofs, inputs);
    Ok(BuiltLayer { json, line })
}

/// One layer's `layers.json` record. `erofs` is `(sha256, size, verity)` of the
/// erofs file as shipped; without erofs the erofs fields and `id` are null.
/// `id` is the erofs sha256 (the board stores layers by it), and the verity
/// fields are those the existing verity transform records.
pub(crate) fn layer_json(
    name: &str,
    tar_sha256: &str,
    tar_size: u64,
    erofs: Option<(String, u64, Option<VerityRecord>)>,
    inputs: &str,
) -> serde_json::Value {
    let (erofs_file, erofs_sha256, erofs_size, id, verity) = match &erofs {
        Some((sha256, size, verity)) => (
            serde_json::json!(format!("{name}.erofs")),
            serde_json::json!(sha256),
            serde_json::json!(size),
            serde_json::json!(sha256),
            verity.as_ref().map_or(serde_json::Value::Null, |record| {
                serde_json::json!({
                    "root_hash": record.root_hash,
                    "salt": record.salt,
                    "hash_offset": record.hash_offset,
                    "data_blocks": record.data_blocks,
                    "data_block_size": VERITY_BLOCK,
                    "hash_block_size": VERITY_BLOCK,
                })
            }),
        ),
        None => (
            serde_json::Value::Null,
            serde_json::Value::Null,
            serde_json::Value::Null,
            serde_json::Value::Null,
            serde_json::Value::Null,
        ),
    };
    serde_json::json!({
        "name": name,
        "id": id,
        "tar": format!("{name}.tar"),
        "tar_sha256": tar_sha256,
        "tar_size": tar_size,
        "erofs": erofs_file,
        "erofs_sha256": erofs_sha256,
        "erofs_size": erofs_size,
        // The names Atlas's layered slot reads: the shipped file's size and
        // sha256 (the same values as erofs_size and erofs_sha256).
        "size": erofs_size,
        "sha256": erofs_sha256,
        "verity": verity,
        "inputs": inputs,
    })
}

/// Writes the layer's entries to `path`, in path order, with each
/// same-layer hardlink's target written first.
fn write_layer_tar(
    path: &Path,
    plan: &[(String, Emit)],
    members: &LayerMembers,
    source: &BTreeMap<String, Entry>,
    rootfs_tar: &Path,
    mtime: u64,
) -> Result<(), ImageProviderError> {
    let file = fs::File::create(path).map_err(|error| {
        runtime_error(format!("failed to create '{}': {error}", path.display()))
    })?;
    let mut data = fs::File::open(rootfs_tar).map_err(|error| {
        runtime_error(format!(
            "failed to open '{}': {error}",
            rootfs_tar.display()
        ))
    })?;
    let mut writer = LayerTarWriter::new(BufWriter::new(file), mtime);
    let index: BTreeMap<&str, &Emit> = plan.iter().map(|(p, e)| (p.as_str(), e)).collect();
    let mut written: BTreeSet<&str> = BTreeSet::new();
    for (path, _) in plan {
        write_one(
            &mut writer,
            path,
            &index,
            members,
            source,
            &mut data,
            &mut written,
        )?;
    }
    let mut out = writer.finish().map_err(runtime_error)?;
    out.flush()
        .map_err(|error| runtime_error(format!("failed to write layer tar: {error}")))?;
    Ok(())
}

fn write_one<'a>(
    writer: &mut LayerTarWriter<BufWriter<fs::File>>,
    path: &'a str,
    index: &BTreeMap<&'a str, &'a Emit>,
    members: &'a LayerMembers,
    source: &'a BTreeMap<String, Entry>,
    data: &mut fs::File,
    written: &mut BTreeSet<&'a str>,
) -> Result<(), ImageProviderError> {
    if written.contains(path) {
        return Ok(());
    }
    let entry = &members[path];
    let emit = index[path];
    if let Emit::Link(target) = emit {
        // The target must precede its links in the archive.
        write_one(
            writer,
            target.as_str(),
            index,
            members,
            source,
            data,
            written,
        )?;
    }
    written.insert(path);
    let result = match (entry.kind, emit) {
        (EntryKind::Dir, _) => writer.dir(path, entry.mode, entry.uid, entry.gid),
        (EntryKind::Symlink, _) => writer.symlink(path, &entry.link, entry.uid, entry.gid),
        (EntryKind::Fifo, _) => writer.fifo(path, entry.mode, entry.uid, entry.gid),
        (EntryKind::CharDev | EntryKind::BlockDev, _) => writer.device(
            path,
            entry.kind == EntryKind::BlockDev,
            entry.mode,
            entry.uid,
            entry.gid,
            (entry.dev_major, entry.dev_minor),
        ),
        (EntryKind::Hardlink, Emit::Link(target)) => {
            writer.hardlink(path, target, entry.mode, entry.uid, entry.gid)
        }
        (EntryKind::File | EntryKind::Hardlink, _) => {
            let (from, size) = match emit {
                Emit::Copy(target) => (&source[target], source[target].size),
                _ => (entry, entry.size),
            };
            writer.file(path, entry.mode, entry.uid, entry.gid, size, |out| {
                copy_data(data, from.offset, size, out)
            })
        }
    };
    result.map_err(runtime_error)
}

pub(crate) fn file_len(path: &Path) -> Result<u64, ImageProviderError> {
    fs::metadata(path)
        .map(|meta| meta.len())
        .map_err(|error| runtime_error(format!("failed to stat '{}': {error}", path.display())))
}

fn publish(temp: &Path, dest: &Path) -> Result<(), ImageProviderError> {
    fs::rename(temp, dest).map_err(|error| {
        let _ = fs::remove_file(temp);
        runtime_error(format!(
            "failed to publish '{}' as '{}': {error}",
            temp.display(),
            dest.display()
        ))
    })
}

/// The `layers.json` of the previous run, when there is one.
fn read_previous(layers_dir: &Path) -> Result<Vec<serde_json::Value>, ImageProviderError> {
    let path = layers_dir.join(LAYERS_JSON);
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    Ok(value["layers"].as_array().cloned().unwrap_or_default())
}

/// Removes layer outputs whose layer is no longer in `order`, and leftovers
/// of an interrupted build.
fn remove_stale_outputs(layers_dir: &Path, order: &[String]) -> Result<(), ImageProviderError> {
    let entries = fs::read_dir(layers_dir).map_err(|error| {
        runtime_error(format!(
            "failed to read '{}': {error}",
            layers_dir.display()
        ))
    })?;
    for entry in entries.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        let stale = file.starts_with('.')
            || [".tar", ".erofs"]
                .iter()
                .find_map(|suffix| file.strip_suffix(suffix))
                .is_some_and(|name| !order.iter().any(|layer| layer == name));
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

/// Writes `layers.json`, its layers in `order`.
fn write_layers_json(
    layers_dir: &Path,
    layers: &ImageLayersSpec,
    built: &[serde_json::Value],
) -> Result<(), ImageProviderError> {
    let document = serde_json::json!({
        "schema": LAYERS_SCHEMA,
        "order": layers.order,
        "layers": built,
    });
    let text = serde_json::to_string_pretty(&document)
        .map_err(|error| runtime_error(format!("failed to encode layers.json: {error}")))?;
    let temp = layers_dir.join(format!(".{LAYERS_JSON}.tmp"));
    fs::write(&temp, format!("{text}\n"))
        .map_err(|error| runtime_error(format!("failed to write layers.json: {error}")))?;
    publish(&temp, &layers_dir.join(LAYERS_JSON))
}
