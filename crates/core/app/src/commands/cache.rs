//! `gaia cache`: inspect and prune the Buildroot package cache levels (the
//! system level shared by every project, and this project's own) entry by
//! entry, so one bad package never means wiping a large cache, and clear a
//! whole level or the compiler cache when that is what is wanted.

use std::fs;
use std::path::{Path, PathBuf};

use gaia_config::{ResolveOptions, try_resolve_config_with_options};
use gaia_spec::{PackageCacheLevelSpec, ResolvedBuildSpec};

use crate::CacheArgs;

use super::CommandOutcome;

/// One cached build of a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry {
    pub level: PackageCacheLevelSpec,
    pub package: String,
    /// The entry's file stem: the package key, with `@<path digest>` for a
    /// build that can only be restored at the path it was built at.
    pub key: String,
    pub version: Option<String>,
    pub bytes: u64,
    pub relocatable: bool,
    /// A `.tar.zst` archive of the first cache format, which Gaia no longer
    /// reads.
    pub legacy: bool,
    /// Paths making up the entry (its directory or archive, and manifest).
    pub paths: Vec<PathBuf>,
}

pub fn cache_command(build: &str, options: &ResolveOptions, args: &CacheArgs) -> CommandOutcome {
    let spec = match try_resolve_config_with_options(build, options) {
        Ok(spec) => spec,
        Err(error) => {
            return CommandOutcome::Failed {
                message: error.to_string(),
            };
        }
    };
    match run_cache_command(&spec, args) {
        Ok(text) => CommandOutcome::Text { text },
        Err(message) => CommandOutcome::Failed { message },
    }
}

fn run_cache_command(spec: &ResolvedBuildSpec, args: &CacheArgs) -> Result<String, String> {
    let level = args
        .level
        .as_deref()
        .map(|level| match level {
            "system" => Ok(PackageCacheLevelSpec::System),
            "project" => Ok(PackageCacheLevelSpec::Project),
            other => Err(format!(
                "--level must be 'system' or 'project', got '{other}'"
            )),
        })
        .transpose()?;
    let levels = package_cache_levels(spec)
        .into_iter()
        .filter(|(candidate, _)| level.is_none_or(|level| level == *candidate))
        .collect::<Vec<_>>();
    let verb = if args.dry_run {
        "would remove"
    } else {
        "removed"
    };
    let mut lines = Vec::new();

    if let Some(target) = args.clear.as_deref() {
        let dirs = match target {
            "system" | "project" => levels
                .iter()
                .filter(|(level, _)| level.as_str() == target)
                .map(|(_, dir)| dir.clone())
                .collect::<Vec<_>>(),
            "ccache" => ccache_dirs(spec),
            other => {
                return Err(format!(
                    "--clear must be 'system', 'project' or 'ccache', got '{other}'"
                ));
            }
        };
        for dir in dirs.into_iter().filter(|dir| dir.exists()) {
            let bytes = tree_bytes(&dir);
            if !args.dry_run {
                gaia_process::discard(&dir)
                    .map_err(|error| format!("failed to remove '{}': {error}", dir.display()))?;
            }
            lines.push(format!("{verb} {} ({})", dir.display(), human(bytes)));
        }
        if lines.is_empty() {
            lines.push(format!("{target} cache: nothing to remove"));
        }
        return Ok(lines.join("\n"));
    }

    let mut entries = levels
        .iter()
        .flat_map(|(level, dir)| cache_entries(*level, dir))
        .filter(|entry| {
            args.packages.is_empty()
                || args
                    .packages
                    .iter()
                    .any(|pattern| gaia_spec::wildcard_match(pattern, &entry.package))
        })
        .collect::<Vec<_>>();

    if !args.remove.is_empty() || args.remove_legacy {
        let mut removed_bytes = 0;
        for entry in &entries {
            let selected = (args.remove_legacy && entry.legacy)
                || args.remove.iter().any(|selector| {
                    let (package, key) =
                        selector.split_once('@').unwrap_or((selector.as_str(), ""));
                    gaia_spec::wildcard_match(package, &entry.package) && entry.key.starts_with(key)
                });
            if !selected {
                continue;
            }
            if !args.dry_run {
                for path in &entry.paths {
                    gaia_process::discard(path).map_err(|error| {
                        format!("failed to remove '{}': {error}", path.display())
                    })?;
                }
            }
            removed_bytes += entry.bytes;
            lines.push(format!(
                "{verb} {} {}@{} ({})",
                entry.level.as_str(),
                entry.package,
                short_key(&entry.key),
                human(entry.bytes)
            ));
        }
        if lines.is_empty() {
            lines.push("no cache entries matched".to_string());
        } else {
            lines.push(format!("{verb} {} in total", human(removed_bytes)));
        }
        return Ok(lines.join("\n"));
    }

    // Listing: largest first, then totals per level.
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.bytes));
    for entry in &entries {
        lines.push(format!(
            "{:<7} {:>9}  {}@{}{}{}",
            entry.level.as_str(),
            human(entry.bytes),
            entry.package,
            short_key(&entry.key),
            entry
                .version
                .as_deref()
                .filter(|version| !version.is_empty())
                .map(|version| format!("  {version}"))
                .unwrap_or_default(),
            if entry.legacy {
                "  (earlier format, no longer read)"
            } else if entry.relocatable {
                ""
            } else {
                "  (restores only at the path it was built at)"
            }
        ));
    }
    let (legacy_count, legacy_bytes) = entries
        .iter()
        .filter(|entry| entry.legacy)
        .fold((0, 0), |(count, bytes), entry| {
            (count + 1, bytes + entry.bytes)
        });
    if legacy_count > 0 {
        lines.push(format!(
            "{legacy_count} entries ({}) are archives of an earlier cache format that is no \
             longer read; 'gaia cache --remove-legacy' removes them",
            human(legacy_bytes)
        ));
    }
    for (level, dir) in &levels {
        let (count, bytes) = entries
            .iter()
            .filter(|entry| entry.level == *level)
            .fold((0, 0), |(count, bytes), entry| {
                (count + 1, bytes + entry.bytes)
            });
        lines.push(format!(
            "{} level: {count} entr{} ({}) in {}",
            level.as_str(),
            if count == 1 { "y" } else { "ies" },
            human(bytes),
            dir.display()
        ));
    }
    Ok(lines.join("\n"))
}

/// The package cache levels of a build, in lookup order: the configured
/// directories, or the defaults Gaia uses (the system level under the user
/// cache root, the project level in the workspace).
pub fn package_cache_levels(spec: &ResolvedBuildSpec) -> Vec<(PackageCacheLevelSpec, PathBuf)> {
    let settings = &spec.policy.providers.buildroot.package_cache;
    let workspace = PathBuf::from(&spec.workspace.root_dir);
    let resolve = |dir: &str| {
        let path = Path::new(dir);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            workspace.join(path)
        }
    };
    let project = settings
        .project_dir
        .as_deref()
        .map(resolve)
        .unwrap_or_else(|| {
            workspace
                .join(".gaia/cache")
                .join(gaia_spec::USER_BUILDROOT_PACKAGE_CACHE_DIR)
        });
    let mut levels = vec![(PackageCacheLevelSpec::Project, project.clone())];
    let system = settings.system_dir.as_deref().map(resolve).or_else(|| {
        gaia_spec::user_cache_root()
            .map(|root| root.join(gaia_spec::USER_BUILDROOT_PACKAGE_CACHE_DIR))
    });
    if let Some(system) = system.filter(|system| *system != project) {
        levels.push((PackageCacheLevelSpec::System, system));
    }
    levels
}

fn ccache_dirs(spec: &ResolvedBuildSpec) -> Vec<PathBuf> {
    let workspace = PathBuf::from(&spec.workspace.root_dir);
    match spec.policy.providers.buildroot.ccache.dir.as_deref() {
        Some(dir) if Path::new(dir).is_absolute() => vec![PathBuf::from(dir)],
        Some(dir) => vec![workspace.join(dir)],
        None => gaia_spec::user_cache_root()
            .map(|root| root.join(gaia_spec::USER_BUILDROOT_CCACHE_DIR))
            .into_iter()
            .chain([workspace
                .join(".gaia/cache")
                .join(gaia_spec::USER_BUILDROOT_CCACHE_DIR)])
            .collect(),
    }
}

/// The entries of one level: `<package>/<key>.json` manifests with their
/// `<key>/` directory (or the `<key>.tar.zst` archive of earlier versions).
pub fn cache_entries(level: PackageCacheLevelSpec, dir: &Path) -> Vec<CacheEntry> {
    let mut entries = Vec::new();
    for package in fs::read_dir(dir).into_iter().flatten().flatten() {
        let Some(package_name) = package.file_name().to_str().map(str::to_string) else {
            continue;
        };
        for file in fs::read_dir(package.path()).into_iter().flatten().flatten() {
            let manifest_path = file.path();
            if manifest_path
                .extension()
                .is_none_or(|extension| extension != "json")
            {
                continue;
            }
            let Some(key) = manifest_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            let manifest = fs::read_to_string(&manifest_path)
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .unwrap_or_default();
            let mut paths = vec![manifest_path.clone()];
            let mut legacy = false;
            let directory = package.path().join(&key);
            let archive = package.path().join(format!("{key}.tar.zst"));
            let bytes = if directory.is_dir() {
                paths.push(directory.clone());
                manifest
                    .get("size")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or_else(|| tree_bytes(&directory))
            } else if archive.is_file() {
                legacy = true;
                paths.push(archive.clone());
                fs::metadata(&archive)
                    .map(|metadata| metadata.len())
                    .unwrap_or(0)
            } else {
                0
            };
            entries.push(CacheEntry {
                level,
                package: package_name.clone(),
                key,
                version: manifest
                    .get("version")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                bytes,
                relocatable: manifest
                    .get("relocatable")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true),
                legacy,
                paths,
            });
        }
    }
    entries
}

fn tree_bytes(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !metadata.is_dir() {
        return metadata.len();
    }
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| tree_bytes(&entry.path()))
        .sum()
}

fn short_key(key: &str) -> &str {
    &key[..key.len().min(12)]
}

fn human(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(root: &Path, level: &str, package: &str, key: &str, size: u64) {
        let dir = root.join(level).join(package);
        fs::create_dir_all(dir.join(key)).expect("entry dir");
        fs::write(dir.join(key).join("file"), vec![0u8; size as usize]).expect("file");
        fs::write(
            dir.join(format!("{key}.json")),
            serde_json::json!({"package": package, "version": "1", "size": size, "relocatable": true})
                .to_string(),
        )
        .expect("manifest");
    }

    fn spec(root: &Path) -> ResolvedBuildSpec {
        let mut spec = ResolvedBuildSpec::new("cache-test");
        spec.workspace.root_dir = root.display().to_string();
        let settings = &mut spec.policy.providers.buildroot.package_cache;
        settings.system_dir = Some(root.join("system").display().to_string());
        settings.project_dir = Some(root.join("project").display().to_string());
        spec
    }

    fn args() -> CacheArgs {
        CacheArgs::default()
    }

    #[test]
    fn single_entries_are_removed_without_touching_the_rest() {
        let root = std::env::temp_dir().join(format!("gaia-cache-cmd-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        entry(&root, "system", "mesa3d", "aaaa1111", 10);
        entry(&root, "system", "mesa3d", "bbbb2222", 20);
        entry(&root, "system", "linux", "cccc3333", 30);
        entry(&root, "project", "photonvision", "dddd4444", 40);
        let spec = spec(&root);

        let listing = run_cache_command(&spec, &args()).expect("list");
        assert!(listing.contains("system level: 3 entries"), "{listing}");
        assert!(listing.contains("project level: 1 entry"), "{listing}");
        assert!(
            listing
                .lines()
                .next()
                .expect("first")
                .contains("photonvision@dddd4444")
        );

        // A dry run removes nothing.
        let dry = run_cache_command(
            &spec,
            &CacheArgs {
                remove: vec!["mesa3d@bbbb".into()],
                dry_run: true,
                ..args()
            },
        )
        .expect("dry run");
        assert!(dry.contains("would remove system mesa3d@bbbb2222"), "{dry}");
        assert!(root.join("system/mesa3d/bbbb2222").is_dir());

        let removed = run_cache_command(
            &spec,
            &CacheArgs {
                remove: vec!["mesa3d@bbbb".into()],
                ..args()
            },
        )
        .expect("remove");
        assert!(
            removed.contains("removed system mesa3d@bbbb2222"),
            "{removed}"
        );
        assert!(!root.join("system/mesa3d/bbbb2222").exists());
        assert!(!root.join("system/mesa3d/bbbb2222.json").exists());
        assert!(root.join("system/mesa3d/aaaa1111").is_dir());
        assert!(root.join("system/linux/cccc3333").is_dir());

        // A whole level.
        run_cache_command(
            &spec,
            &CacheArgs {
                clear: Some("project".into()),
                ..args()
            },
        )
        .expect("clear");
        assert!(!root.join("project").exists());
        assert!(root.join("system/linux").is_dir());
        let _ = fs::remove_dir_all(root);
    }
}
