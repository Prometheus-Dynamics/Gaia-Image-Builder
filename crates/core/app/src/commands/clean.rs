use gaia_config::{ResolveOptions, try_resolve_config_with_options};
use gaia_spec::{ArtifactDefinition, CleanProfileSpec, ResolvedBuildSpec, SourceDefinition};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::CleanArgs;

use super::CommandOutcome;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanReport {
    pub build_name: String,
    pub dry_run: bool,
    pub removed: Vec<PathBuf>,
    pub missing: Vec<PathBuf>,
    /// Sizes of removed cache paths (measured for the `caches` target only;
    /// walking a whole build tree just to report its size is too slow).
    pub sizes: BTreeMap<PathBuf, u64>,
}

impl CleanReport {
    pub fn freed_bytes(&self) -> u64 {
        self.sizes.values().sum()
    }
}

pub(crate) fn clean_build_command(
    build: &str,
    options: &ResolveOptions,
    clean_args: &CleanArgs,
) -> CommandOutcome {
    let spec = match try_resolve_config_with_options(build, options) {
        Ok(spec) => spec,
        Err(error) => {
            return CommandOutcome::Failed {
                message: error.to_string(),
            };
        }
    };

    match clean_build(&spec, clean_args) {
        Ok(report) => CommandOutcome::Cleaned { spec, report },
        Err(message) => CommandOutcome::Failed { message },
    }
}

fn clean_build(spec: &ResolvedBuildSpec, clean_args: &CleanArgs) -> Result<CleanReport, String> {
    let paths = clean_paths(spec, clean_args)?;
    let cache_paths = cache_clean_paths(spec, clean_args)?;
    let mut removed = Vec::new();
    let mut missing = Vec::new();
    let mut sizes = BTreeMap::new();

    let tagged = paths
        .into_iter()
        .map(|path| (path, false))
        .chain(cache_paths.into_iter().map(|path| (path, true)));
    for (path, is_cache) in tagged {
        guard_clean_path(spec, &path)?;
        // Already removed, or inside a directory that was.
        if removed.iter().any(|done: &PathBuf| path.starts_with(done)) {
            continue;
        }
        // A link whose target is gone (a RAM tree after a reboot) is still
        // removed.
        if !path.exists() && fs::symlink_metadata(&path).is_err() {
            if !missing.contains(&path) {
                missing.push(path);
            }
            continue;
        }
        if is_cache {
            sizes.insert(path.clone(), path_size(&path));
        }
        if !clean_args.dry_run {
            remove_path(&path).map_err(|error| {
                format!(
                    "failed to clean '{}' for build '{}': {error}",
                    path.display(),
                    spec.identity.display_name
                )
            })?;
        }
        removed.push(path);
    }

    Ok(CleanReport {
        build_name: spec.identity.display_name.clone(),
        dry_run: clean_args.dry_run,
        removed,
        sizes,
        missing,
    })
}

fn clean_paths(spec: &ResolvedBuildSpec, clean_args: &CleanArgs) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();

    if let Some(profile_name) = clean_args.profile.as_deref() {
        append_profile_paths(spec, profile_name, &mut paths)?;
    } else if clean_args.targets.is_empty() && clean_args.paths.is_empty() && !clean_args.all_caches
    {
        if let Some(profile_name) = spec.clean.default_profile.as_deref() {
            append_profile_paths(spec, profile_name, &mut paths)?;
        } else {
            paths.push(PathBuf::from(&spec.workspace.build_dir));
            paths.push(PathBuf::from(&spec.workspace.out_dir));
        }
    }

    for target in &clean_args.targets {
        append_target_paths(spec, target, &mut paths)?;
    }

    for path in &clean_args.paths {
        paths.push(spec.workspace.resolve_path(path).map_err(|error| {
            format!(
                "invalid clean path '{}' for build '{}': {error}",
                path, spec.identity.display_name
            )
        })?);
    }

    Ok(dedupe_paths(paths))
}

fn append_profile_paths(
    spec: &ResolvedBuildSpec,
    profile_name: &str,
    paths: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let profile = spec.clean.profiles.get(profile_name).ok_or_else(|| {
        format!(
            "unknown clean profile '{}' for build '{}'",
            profile_name, spec.identity.display_name
        )
    })?;
    append_profile(spec, profile, paths)
}

fn append_profile(
    spec: &ResolvedBuildSpec,
    profile: &CleanProfileSpec,
    paths: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if profile.build {
        paths.push(PathBuf::from(&spec.workspace.build_dir));
    }
    if profile.out {
        paths.push(PathBuf::from(&spec.workspace.out_dir));
    }
    for path in &profile.paths {
        paths.push(spec.workspace.resolve_path(path).map_err(|error| {
            format!(
                "invalid configured clean path '{}' for build '{}': {error}",
                path, spec.identity.display_name
            )
        })?);
    }
    Ok(())
}

fn append_target_paths(
    spec: &ResolvedBuildSpec,
    target: &str,
    paths: &mut Vec<PathBuf>,
) -> Result<(), String> {
    match target {
        "build" => paths.push(PathBuf::from(&spec.workspace.build_dir)),
        // Collected by `cache_clean_paths`.
        "caches" => {}
        "out" | "outputs" => paths.push(PathBuf::from(&spec.workspace.out_dir)),
        "all" => {
            paths.push(PathBuf::from(&spec.workspace.build_dir));
            paths.push(PathBuf::from(&spec.workspace.out_dir));
        }
        "configured" => {
            let Some(profile_name) = spec.clean.default_profile.as_deref() else {
                return Err(format!(
                    "clean target 'configured' requires clean.default for build '{}'",
                    spec.identity.display_name
                ));
            };
            append_profile_paths(spec, profile_name, paths)?;
        }
        value => {
            return Err(format!(
                "unknown clean target '{}' for build '{}'",
                value, spec.identity.display_name
            ));
        }
    }
    Ok(())
}

/// Workspace-relative caches that `--all-caches` removes wholesale. They are
/// shared by every build in the workspace and refill on demand.
const SHARED_CACHE_DIRS: &[&str] = &[
    gaia_source_providers::GIT_MIRROR_CACHE_DIR,
    gaia_source_providers::DOWNLOAD_CACHE_DIR,
    ".gaia/cache/buildroot/dl",
    ".gaia/docker-cache",
    ".gaia/cache/import-sources",
    ".gaia/docker-home",
];

/// Name of the directory inside a shared cargo target dir that records which
/// source directory each local package was built from (see the rust provider).
const SHARED_OWNERS_DIR: &str = "gaia-owners";

/// Paths removed by the `caches` target (or `--all-caches`):
/// - `.<source>.gaia-preserved` stashes left by an interrupted re-clone,
/// - Buildroot `target.refresh` work trees left by squashfs refreshes,
/// - stale shared cargo target dirs under the user cache (see
///   [`stale_shared_cargo_targets`]),
/// - with `--unused-mirrors`, git mirrors under `.gaia/cache/git` that no git
///   source or cargo git dependency of this build uses,
/// - with `--all-caches`, every git mirror and the shared caches in
///   [`SHARED_CACHE_DIRS`].
///
/// Git mirrors are shared by every build in the workspace, so they are pruned
/// only on request: a build cannot see the mirrors another build still needs.
fn cache_clean_paths(
    spec: &ResolvedBuildSpec,
    clean_args: &CleanArgs,
) -> Result<Vec<PathBuf>, String> {
    let wants_caches =
        clean_args.all_caches || clean_args.targets.iter().any(|target| target == "caches");
    if !wants_caches {
        return Ok(Vec::new());
    }
    let workspace_root = PathBuf::from(&spec.workspace.root_dir);
    // Source providers key the cache off the canonical workspace root.
    let cache_root = fs::canonicalize(&workspace_root).unwrap_or(workspace_root);
    let mut paths = Vec::new();

    if clean_args.all_caches {
        paths.extend(SHARED_CACHE_DIRS.iter().map(|dir| cache_root.join(dir)));
    } else if clean_args.unused_mirrors {
        paths.extend(unused_mirror_paths(spec, &cache_root));
    }
    let user_cache = gaia_spec::user_cache_root();
    paths.extend(stale_shared_cargo_targets(user_cache.as_deref()));

    let sources_dir = PathBuf::from(&spec.workspace.build_dir).join("sources");
    paths.extend(sorted_children(&sources_dir).into_iter().filter(|path| {
        path_name(path)
            .is_some_and(|name| name.starts_with('.') && name.ends_with(".gaia-preserved"))
    }));
    let buildroot_fs =
        PathBuf::from(&spec.workspace.build_dir).join("image/buildroot-output/build/buildroot-fs");
    paths.extend(
        sorted_children(&buildroot_fs)
            .into_iter()
            .map(|fs_dir| fs_dir.join("target.refresh"))
            .filter(|path| path.is_dir()),
    );
    Ok(dedupe_paths(paths))
}

/// Git mirrors under `.gaia/cache/git` that no git source of this build
/// fetches and no cargo git dependency of this build maps to. Includes the
/// mirrors of the old one-per-ref layout.
fn unused_mirror_paths(spec: &ResolvedBuildSpec, cache_root: &Path) -> Vec<PathBuf> {
    let mirrors = cache_root.join(gaia_source_providers::GIT_MIRROR_CACHE_DIR);
    let mut repos = Vec::new();
    for source in &spec.sources {
        if let SourceDefinition::Git(git) = &source.definition {
            repos.push(git.repo.as_str());
        }
    }
    for artifact in &spec.artifacts {
        if let ArtifactDefinition::Rust(rust) = &artifact.definition {
            repos.extend(
                rust.cargo_git_sources
                    .iter()
                    .map(|mapping| mapping.repo.as_str()),
            );
        }
    }
    // Resolving a used mirror renames one still under its legacy name, so it
    // is kept rather than cleaned as unused.
    let used = repos
        .into_iter()
        .filter_map(|repo| {
            gaia_source_providers::remote_git_mirror_dir(&mirrors, repo)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .collect::<BTreeSet<_>>();
    sorted_children(&mirrors)
        .into_iter()
        .filter(|path| !path_name(path).is_some_and(|name| used.contains(name)))
        .collect()
}

/// Shared cargo target dirs (`<user cache>/cargo-target/<key>`) that no live
/// source owns: every package claim in their `gaia-owners` directory names a
/// source directory that no longer exists. A dir with no claims is kept, since
/// a build may be filling it right now, and so is a dir whose owner exists.
fn stale_shared_cargo_targets(user_cache: Option<&Path>) -> Vec<PathBuf> {
    let Some(root) = user_cache else {
        return Vec::new();
    };
    sorted_children(&root.join("cargo-target"))
        .into_iter()
        .filter(|dir| {
            let claims = sorted_children(&dir.join(SHARED_OWNERS_DIR));
            !claims.is_empty()
                && claims.iter().all(|claim| {
                    fs::read_to_string(claim)
                        .is_ok_and(|owner| !owner.is_empty() && !Path::new(&owner).exists())
                })
        })
        .collect()
}

fn sorted_children(dir: &Path) -> Vec<PathBuf> {
    let mut children = fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    children.sort();
    children
}

fn path_name(path: &Path) -> Option<&str> {
    path.file_name().and_then(|name| name.to_str())
}

/// Total size of the files under `path` (symlinks are not followed).
fn path_size(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !metadata.is_dir() {
        return metadata.len();
    }
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(entry.path()),
                Ok(_) => total += entry.metadata().map(|meta| meta.len()).unwrap_or(0),
                Err(_) => {}
            }
        }
    }
    total
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();
    for path in paths {
        if seen.insert(path.clone()) {
            deduped.push(path);
        }
    }
    deduped
}

fn guard_clean_path(spec: &ResolvedBuildSpec, path: &Path) -> Result<(), String> {
    let workspace_root = Path::new(&spec.workspace.root_dir);
    if path == workspace_root {
        return Err(format!(
            "refusing to clean workspace root '{}' for build '{}'",
            path.display(),
            spec.identity.display_name
        ));
    }
    if path.parent().is_none() {
        return Err(format!(
            "refusing to clean unsafe path '{}' for build '{}'",
            path.display(),
            spec.identity.display_name
        ));
    }
    Ok(())
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    // A Buildroot tree built in RAM or a work dir is a link to it: remove
    // the tree too, not only the link.
    for link in [path.to_path_buf(), path.join("image/buildroot-output")] {
        if let Ok(target) = fs::read_link(&link)
            && target.ends_with("buildroot-output")
        {
            gaia_process::discard(&target)?;
        }
    }
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        gaia_process::discard(path)
    } else {
        fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::{SHARED_OWNERS_DIR, stale_shared_cargo_targets};
    use gaia_testkit::TempDir;
    use std::fs;

    #[test]
    fn shared_cargo_target_is_stale_only_when_every_owner_is_gone() {
        let scratch = TempDir::new("gaia-clean-cargo-targets");
        let cache = scratch.join("cache");
        let targets = cache.join("cargo-target");
        let live_source = scratch.join("live-source");
        fs::create_dir_all(&live_source).expect("live source");
        let claim_dir = |key: &str| targets.join(key).join(SHARED_OWNERS_DIR);
        let claim = |key: &str, name: &str, owner: &std::path::Path| {
            fs::create_dir_all(claim_dir(key)).expect("owners dir");
            fs::write(claim_dir(key).join(name), owner.display().to_string()).expect("claim");
        };
        // Every owner is gone: stale.
        claim("gone", "worker-0.1.0", &scratch.join("removed-source"));
        // One owner still exists: kept.
        claim("live", "worker-0.1.0", &scratch.join("removed-source"));
        claim("live", "vega-0.1.0", &live_source);
        // No claims at all (a build may be filling it): kept.
        fs::create_dir_all(targets.join("unclaimed")).expect("unclaimed");

        let stale = stale_shared_cargo_targets(Some(&cache));
        assert_eq!(stale, vec![targets.join("gone")], "{stale:?}");
        assert!(stale_shared_cargo_targets(None).is_empty());
    }
}
