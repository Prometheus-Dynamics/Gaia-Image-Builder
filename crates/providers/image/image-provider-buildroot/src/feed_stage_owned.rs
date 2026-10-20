//! Ownership of the target paths that `[[stage.files]]` entries install.
//!
//! A stage entry copies its source into the Buildroot target, and a copy
//! never removes what an earlier copy left behind. So after each successful
//! apply, Gaia records what every stage entry installed: its paths, their
//! kinds, and for files the size and modification time the copy left
//! (`copy_path` and `cp -a` keep the modification time). The next apply
//! removes what an entry owned last time and no longer produces, before it
//! copies the new content. A removed entry's paths go the same way.
//!
//! A path is removed only when the target still matches what the stage
//! wrote. A file a package rewrote, a path that another entry or the feed
//! now produces, and a directory that is not empty are all kept. Nothing
//! the stage did not record is ever touched.
use super::*;
use std::os::unix::fs::MetadataExt;

const STAGE_OWNED_FILE: &str = ".gaia-image-feed-stage-owned.txt";
const STAGE_OWNED_HEADER: &str = "gaia-image-feed-stage-owned-v1";

/// What a stage entry's source makes in the target, before the copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PlannedKind {
    Dir,
    File,
    Link(String),
}

/// One stage entry and the target paths its source produces (relative to the
/// target root, no leading slash), its parent directories included.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StageItemPlan {
    pub(crate) id: String,
    pub(crate) paths: Vec<(String, PlannedKind)>,
}

/// The stage entries of an image feed and every target path the whole feed
/// produces.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StageOwnership {
    pub(crate) items: Vec<StageItemPlan>,
    /// Every path the feed produces (stage entries, installs, env sets and
    /// services) with its parent directories. Nothing here is ever removed.
    pub(crate) produced: BTreeSet<String>,
}

/// What an entry left in the target, as recorded after a successful apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OwnedKind {
    Dir,
    File { size: u64, mtime_ns: i128 },
    Link(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OwnedRecord {
    pub(crate) item: String,
    pub(crate) rel: String,
    pub(crate) kind: OwnedKind,
}

pub(crate) fn stage_owned_path(output_dir: &Path) -> PathBuf {
    output_dir.join(STAGE_OWNED_FILE)
}

/// Plans the stage entries of `image`: what each source copies to, and what
/// the whole feed produces. Reads the sources, not the target.
pub(crate) fn plan_stage_ownership(
    spec: &ResolvedBuildSpec,
    image: &ImageSpec,
) -> Result<StageOwnership, ImageProviderError> {
    let mut ownership = StageOwnership::default();
    for path in image_feed_managed_paths(spec, image)? {
        add_with_ancestors(&mut ownership.produced, &path);
    }
    for stage_file_id in &image.feed.stage_files {
        let stage_file = spec
            .stage
            .files
            .iter()
            .find(|file| file.id == *stage_file_id)
            .ok_or_else(|| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "image feed references unknown stage file '{}'",
                        stage_file_id.as_str()
                    ),
                )
            })?;
        let src = resolve_workspace_path(spec, &stage_file.src)?;
        let dest = normalize_rel(&stage_file.dest);
        let mut paths = Vec::new();
        plan_source(&src, &dest, &mut paths)?;
        // The copy creates the destination's parents: they are the entry's too.
        let mut parent = String::new();
        for part in dest.split('/').take(dest.matches('/').count()) {
            if !parent.is_empty() {
                parent.push('/');
            }
            parent.push_str(part);
            if !paths.iter().any(|(rel, _)| *rel == parent) {
                paths.push((parent.clone(), PlannedKind::Dir));
            }
        }
        for (rel, _) in &paths {
            add_with_ancestors(&mut ownership.produced, rel);
        }
        ownership.items.push(StageItemPlan {
            id: stage_file.id.as_str().to_string(),
            paths,
        });
    }
    Ok(ownership)
}

fn plan_source(
    src: &Path,
    rel: &str,
    out: &mut Vec<(String, PlannedKind)>,
) -> Result<(), ImageProviderError> {
    check_trackable(rel)?;
    let metadata = fs::symlink_metadata(src).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!("failed to stat stage source '{}': {error}", src.display()),
        )
    })?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(src).map_err(|error| {
            ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!("failed to read symlink '{}': {error}", src.display()),
            )
        })?;
        out.push((
            rel.to_string(),
            PlannedKind::Link(target.to_string_lossy().into_owned()),
        ));
    } else if metadata.is_dir() {
        out.push((rel.to_string(), PlannedKind::Dir));
        let mut children = fs::read_dir(src)
            .map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!("failed to read stage source '{}': {error}", src.display()),
                )
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!("failed to read stage source '{}': {error}", src.display()),
                )
            })?;
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let name = child.file_name().to_string_lossy().into_owned();
            plan_source(&child.path(), &format!("{rel}/{name}"), out)?;
        }
    } else {
        out.push((rel.to_string(), PlannedKind::File));
    }
    Ok(())
}

fn check_trackable(rel: &str) -> Result<(), ImageProviderError> {
    if rel.contains(['\t', '\n']) {
        return Err(ImageProviderError::new(
            ImageProviderErrorKind::PolicyBlocked,
            format!("stage path '{rel}' contains a tab or newline, which Gaia cannot track"),
        ));
    }
    Ok(())
}

fn normalize_rel(path: &str) -> String {
    path.trim_matches('/').to_string()
}

fn add_with_ancestors(set: &mut BTreeSet<String>, rel: &str) {
    let rel = normalize_rel(rel);
    let mut current = String::new();
    for part in rel.split('/').filter(|part| !part.is_empty()) {
        if !current.is_empty() {
            current.push('/');
        }
        current.push_str(part);
        set.insert(current.clone());
    }
}

/// Records what each stage entry left in `target`, after the feed was
/// applied. A path two entries produce belongs to the later one, as in the
/// copy order. A planned path missing from the target is not recorded.
pub(crate) fn record_stage_ownership(
    output_dir: &Path,
    ownership: &StageOwnership,
    target: &Path,
) -> Result<(), ImageProviderError> {
    let mut owners: BTreeMap<&str, (&str, &PlannedKind)> = BTreeMap::new();
    for item in &ownership.items {
        for (rel, kind) in &item.paths {
            owners.insert(rel.as_str(), (item.id.as_str(), kind));
        }
    }
    let mut body = format!("{STAGE_OWNED_HEADER}\n");
    for (rel, (item, kind)) in owners {
        let Some(stamp) = stamp_target_path(&target.join(rel), kind)? else {
            continue;
        };
        body.push_str(item);
        body.push('\t');
        body.push_str(rel);
        match stamp {
            OwnedKind::Dir => body.push_str("\tdir"),
            OwnedKind::File { size, mtime_ns } => {
                body.push_str(&format!("\tfile\t{size}\t{mtime_ns}"));
            }
            OwnedKind::Link(link) => {
                body.push_str("\tlink\t");
                body.push_str(&link);
            }
        }
        body.push('\n');
    }
    let path = stage_owned_path(output_dir);
    fs::write(&path, body).map_err(|error| {
        ImageProviderError::new(
            ImageProviderErrorKind::RuntimeState,
            format!(
                "failed to write stage ownership record '{}': {error}",
                path.display()
            ),
        )
    })
}

/// The state a path has in the target now, if it is still the kind planned.
fn stamp_target_path(
    path: &Path,
    planned: &PlannedKind,
) -> Result<Option<OwnedKind>, ImageProviderError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to inspect stage target '{}': {error}",
                    path.display()
                ),
            ));
        }
    };
    let file_type = metadata.file_type();
    Ok(match planned {
        PlannedKind::Dir if file_type.is_dir() => Some(OwnedKind::Dir),
        PlannedKind::File if file_type.is_file() => Some(OwnedKind::File {
            size: metadata.len(),
            mtime_ns: i128::from(metadata.mtime()) * 1_000_000_000
                + i128::from(metadata.mtime_nsec()),
        }),
        PlannedKind::Link(_) if file_type.is_symlink() => {
            let target = fs::read_link(path).map_err(|error| {
                ImageProviderError::new(
                    ImageProviderErrorKind::RuntimeState,
                    format!(
                        "failed to read stage target link '{}': {error}",
                        path.display()
                    ),
                )
            })?;
            Some(OwnedKind::Link(target.to_string_lossy().into_owned()))
        }
        _ => None,
    })
}

/// The records of the last successful apply, or `None` when no record exists
/// yet (the first run after this scheme, or a target built without a feed).
pub(crate) fn read_stage_ownership(
    output_dir: &Path,
) -> Result<Option<Vec<OwnedRecord>>, ImageProviderError> {
    let path = stage_owned_path(output_dir);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ImageProviderError::new(
                ImageProviderErrorKind::RuntimeState,
                format!(
                    "failed to read stage ownership record '{}': {error}",
                    path.display()
                ),
            ));
        }
    };
    let mut records = Vec::new();
    for line in contents.lines().skip(1) {
        let fields = line.split('\t').collect::<Vec<_>>();
        let kind = match fields.as_slice() {
            [_, _, "dir"] => Some(OwnedKind::Dir),
            [_, _, "file", size, mtime] => match (size.parse(), mtime.parse()) {
                (Ok(size), Ok(mtime_ns)) => Some(OwnedKind::File { size, mtime_ns }),
                _ => None,
            },
            [_, _, "link", target] => Some(OwnedKind::Link((*target).to_string())),
            _ => None,
        };
        match kind {
            Some(kind) => records.push(OwnedRecord {
                item: fields[0].to_string(),
                rel: fields[1].to_string(),
                kind,
            }),
            None => tracing::warn!(
                provider_domain = "image.buildroot",
                "skipped an unreadable line in '{}'",
                path.display()
            ),
        }
    }
    Ok(Some(records))
}

/// What [`prune_stale_stage_ownership`] removed and kept.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct StagePrune {
    pub(crate) removed: usize,
    /// Paths a stage entry owned that changed since it wrote them.
    pub(crate) kept_changed: Vec<String>,
}

/// Removes, from `target`, what the recorded entries own and `produced` no
/// longer makes. Files and links go only when they still match the recorded
/// stamp; directories go only when empty, deepest first.
pub(crate) fn prune_stale_stage_ownership(
    records: &[OwnedRecord],
    produced: &BTreeSet<String>,
    target: &Path,
) -> Result<StagePrune, ImageProviderError> {
    let mut report = StagePrune::default();
    let mut dirs = Vec::new();
    for record in records {
        if produced.contains(&record.rel) {
            continue;
        }
        let path = target.join(&record.rel);
        match &record.kind {
            OwnedKind::Dir => dirs.push(record.rel.clone()),
            kind => {
                let current = match fs::symlink_metadata(&path) {
                    Ok(_) => stamp_for_compare(&path, kind)?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(ImageProviderError::new(
                            ImageProviderErrorKind::RuntimeState,
                            format!(
                                "failed to inspect stale stage path '{}': {error}",
                                path.display()
                            ),
                        ));
                    }
                };
                if current.as_ref() == Some(kind) {
                    fs::remove_file(&path).map_err(|error| {
                        ImageProviderError::new(
                            ImageProviderErrorKind::RuntimeState,
                            format!(
                                "failed to remove stale stage path '{}': {error}",
                                path.display()
                            ),
                        )
                    })?;
                    report.removed += 1;
                } else {
                    report.kept_changed.push(record.rel.clone());
                }
            }
        }
    }
    dirs.sort_by(|a, b| {
        b.matches('/')
            .count()
            .cmp(&a.matches('/').count())
            .then_with(|| b.cmp(a))
    });
    for rel in dirs {
        let path = target.join(&rel);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => match fs::remove_dir(&path) {
                Ok(()) => report.removed += 1,
                Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
                Err(error) => {
                    return Err(ImageProviderError::new(
                        ImageProviderErrorKind::RuntimeState,
                        format!(
                            "failed to remove stale stage directory '{}': {error}",
                            path.display()
                        ),
                    ));
                }
            },
            _ => {}
        }
    }
    Ok(report)
}

/// The stamp of a path that a record describes, read the same way it was
/// written. `None` when the path is no longer that kind of entry.
fn stamp_for_compare(
    path: &Path,
    recorded: &OwnedKind,
) -> Result<Option<OwnedKind>, ImageProviderError> {
    let planned = match recorded {
        OwnedKind::Dir => PlannedKind::Dir,
        OwnedKind::File { .. } => PlannedKind::File,
        OwnedKind::Link(target) => PlannedKind::Link(target.clone()),
    };
    stamp_target_path(path, &planned)
}

/// Prunes the target against the records of the last apply, for the paths
/// `ownership` no longer produces, and returns the build messages. Without a
/// record it removes nothing.
pub(crate) fn prune_stage_ownership(
    output_dir: &Path,
    ownership: &StageOwnership,
    target: &Path,
) -> Result<Vec<String>, ImageProviderError> {
    let Some(records) = read_stage_ownership(output_dir)? else {
        return Ok(Vec::new());
    };
    let report = prune_stale_stage_ownership(&records, &ownership.produced, target)?;
    let mut messages = Vec::new();
    if report.removed > 0 {
        messages.push(format!(
            "removed {} staged path(s) that no stage entry produces any more",
            report.removed
        ));
    }
    if !report.kept_changed.is_empty() {
        let shown = report
            .kept_changed
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        messages.push(format!(
            "kept {} path(s) a stage entry wrote but changed since (e.g. {shown})",
            report.kept_changed.len()
        ));
    }
    Ok(messages)
}

/// Whether the output has a stage ownership record.
pub(crate) fn has_stage_ownership_record(output_dir: &Path) -> bool {
    stage_owned_path(output_dir).is_file()
}

/// The note for the first apply after this scheme, when a target exists that
/// no record describes.
pub(crate) const NO_STAGE_OWNERSHIP_NOTE: &str =
    "no stage ownership record yet; leftovers from earlier runs are not removed";

/// For an image without a feed: removes what the last apply's stage entries
/// still own from `target`, then the record. Returns the build messages.
pub(crate) fn clear_stage_ownership(
    output_dir: &Path,
    target: &Path,
) -> Result<Vec<String>, ImageProviderError> {
    if !has_stage_ownership_record(output_dir) {
        return Ok(Vec::new());
    }
    let mut messages = Vec::new();
    if target.is_dir() {
        invalidate_finalized(output_dir);
        messages = prune_stage_ownership(output_dir, &StageOwnership::default(), target)?;
    }
    remove_path_if_exists(&stage_owned_path(output_dir))?;
    Ok(messages)
}
