//! `gaia cache stats`: every cache Gaia keeps for a build, with its path and
//! disk usage (see `disk_usage`), and the total. Workspace caches live under
//! `<workspace>/.gaia`; user-wide ones under the user cache root
//! (`$GAIA_CACHE_DIR`, else `$XDG_CACHE_HOME/gaia`, else `~/.cache/gaia`).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use gaia_spec::ResolvedBuildSpec;

use super::cache::{CacheReport, ccache_dirs, human, package_cache_levels};
use super::disk_usage::{DiskUsage, TreeUsage, tree_usage};

/// One cache directory and what it holds.
struct CacheDir {
    name: String,
    path: PathBuf,
    usage: Option<TreeUsage>,
}

/// A spec for the current directory, for `gaia cache stats` without a build
/// config that resolves: the user-wide caches and the workspace defaults.
pub(super) fn workspace_spec() -> ResolvedBuildSpec {
    let mut spec = ResolvedBuildSpec::new("cache-stats");
    spec.workspace.root_dir = std::env::current_dir()
        .map(|dir| dir.display().to_string())
        .unwrap_or_else(|_| ".".into());
    spec
}

pub(super) fn cache_stats(spec: &ResolvedBuildSpec) -> CacheReport {
    cache_stats_in(spec, gaia_spec::user_cache_root().as_deref())
}

/// The caches of `spec`, in a fixed order, each path once.
fn cache_dirs(spec: &ResolvedBuildSpec, user_root: Option<&Path>) -> Vec<(String, PathBuf)> {
    let root = PathBuf::from(&spec.workspace.root_dir);
    // Source providers key the workspace caches off the canonical root.
    let workspace = fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
    let resolve = |dir: &str| {
        let path = Path::new(dir);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        }
    };
    let buildroot = &spec.policy.providers.buildroot;
    let mut dirs = vec![
        (
            "git-mirrors".to_string(),
            workspace.join(gaia_source_providers::GIT_MIRROR_CACHE_DIR),
        ),
        (
            "downloads".into(),
            workspace.join(gaia_source_providers::DOWNLOAD_CACHE_DIR),
        ),
        (
            "import-sources".into(),
            workspace.join(".gaia/cache/import-sources"),
        ),
    ];
    match buildroot.download_dir.as_deref() {
        Some(dir) => dirs.push(("buildroot-dl".into(), resolve(dir))),
        None => {
            if let Some(user_root) = user_root {
                dirs.push(("buildroot-dl".into(), user_root.join("buildroot/dl")));
            }
            dirs.push((
                "buildroot-dl (workspace)".into(),
                workspace.join(".gaia/cache/buildroot/dl"),
            ));
        }
    }
    for (level, dir) in package_cache_levels(spec) {
        dirs.push((format!("buildroot-packages ({})", level.as_str()), dir));
    }
    dirs.push((
        "buildroot-shared-output".into(),
        buildroot
            .shared_output_dir
            .as_deref()
            .map(resolve)
            .unwrap_or_else(|| workspace.join(".gaia/cache/buildroot/shared")),
    ));
    let ccache = ccache_dirs(spec);
    let several = ccache.len() > 1;
    for dir in ccache {
        let name = if several && !user_root.is_some_and(|user_root| dir.starts_with(user_root)) {
            "ccache (workspace)".to_string()
        } else {
            "ccache".to_string()
        };
        dirs.push((name, dir));
    }
    dirs.push(("docker-cache".into(), workspace.join(".gaia/docker-cache")));
    dirs.push(("docker-home".into(), workspace.join(".gaia/docker-home")));
    if let Some(user_root) = user_root {
        let cargo = user_root.join("cargo-target");
        let mut shared = fs::read_dir(&cargo)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        shared.sort();
        if shared.is_empty() {
            dirs.push(("cargo-target".into(), cargo));
        }
        for dir in shared {
            let key = dir
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            dirs.push((format!("cargo-target/{key}"), dir));
        }
        dirs.push(("gradle-home".into(), user_root.join("gradle-home")));
        dirs.push(("java-home".into(), user_root.join("java-home")));
        // The user-wide defaults, also when this build's config points
        // elsewhere: other builds use them.
        for (name, relative) in [
            ("buildroot-dl (user)", "buildroot/dl"),
            (
                "buildroot-packages (user)",
                gaia_spec::USER_BUILDROOT_PACKAGE_CACHE_DIR,
            ),
            ("ccache (user)", gaia_spec::USER_BUILDROOT_CCACHE_DIR),
        ] {
            dirs.push((name.into(), user_root.join(relative)));
        }
    }
    let mut seen = BTreeSet::new();
    dirs.retain(|(_, path)| seen.insert(path.clone()));
    dirs
}

fn cache_stats_in(spec: &ResolvedBuildSpec, user_root: Option<&Path>) -> CacheReport {
    let caches = cache_dirs(spec, user_root)
        .into_iter()
        .map(|(name, path)| CacheDir {
            usage: tree_usage(&path),
            name,
            path,
        })
        .collect::<Vec<_>>();
    // A cache inside another listed one is already in that one's total.
    let nested = |cache: &CacheDir| {
        caches.iter().any(|outer| {
            outer.path != cache.path && outer.usage.is_some() && cache.path.starts_with(&outer.path)
        })
    };
    let mut total = TreeUsage::default();
    for cache in caches.iter().filter(|cache| !nested(cache)) {
        if let Some(usage) = &cache.usage {
            total.merge(usage);
        }
    }
    let total = total.total();

    let width = caches
        .iter()
        .map(|cache| cache.name.len())
        .max()
        .unwrap_or(0);
    let mut lines = caches
        .iter()
        .map(|cache| match &cache.usage {
            Some(usage) => {
                let usage = usage.total();
                format!(
                    "{:<width$}  {:>10}  {:>9} files  {}",
                    cache.name,
                    human(usage.bytes),
                    usage.files,
                    cache.path.display()
                )
            }
            None => format!(
                "{:<width$}  {:>10}  {:>9}        {}  (not present)",
                cache.name,
                "-",
                "",
                cache.path.display()
            ),
        })
        .collect::<Vec<_>>();
    let present = caches.iter().filter(|cache| cache.usage.is_some()).count();
    lines.push(format!(
        "total: {} in {} files, {present} of {} caches present (hard links counted once)",
        human(total.bytes),
        total.files,
        caches.len()
    ));
    let json = serde_json::json!({
        "action": "stats",
        "caches": caches.iter().map(|cache| {
            let usage = cache.usage.as_ref().map(TreeUsage::total).unwrap_or_default();
            serde_json::json!({
                "name": cache.name,
                "path": cache.path.display().to_string(),
                "exists": cache.usage.is_some(),
                "nested": nested(cache),
                "bytes": usage.bytes,
                "apparent_bytes": usage.apparent_bytes,
                "files": usage.files,
                "dirs": usage.dirs,
            })
        }).collect::<Vec<_>>(),
        "total": usage_json(&total),
    });
    CacheReport { lines, json }
}

fn usage_json(usage: &DiskUsage) -> serde_json::Value {
    serde_json::json!({
        "bytes": usage.bytes,
        "apparent_bytes": usage.apparent_bytes,
        "files": usage.files,
        "dirs": usage.dirs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_testkit::TempDir;

    #[test]
    fn stats_list_every_cache_with_usage_and_a_deduplicated_total() {
        let scratch = TempDir::new("gaia-cache-stats");
        let workspace = scratch.join("ws");
        let user = scratch.join("user");
        fs::create_dir_all(workspace.join(".gaia/cache/git/repo.git")).expect("git");
        fs::write(
            workspace.join(".gaia/cache/git/repo.git/pack"),
            vec![0u8; 8192],
        )
        .expect("pack");
        fs::create_dir_all(workspace.join(".gaia/docker-home")).expect("home");
        fs::create_dir_all(user.join("cargo-target/abc123")).expect("cargo");
        fs::write(user.join("cargo-target/abc123/lib.rlib"), vec![0u8; 4096]).expect("rlib");
        // The same file in two caches counts once in the total.
        fs::create_dir_all(user.join("buildroot/dl")).expect("dl");
        fs::hard_link(
            workspace.join(".gaia/cache/git/repo.git/pack"),
            user.join("buildroot/dl/pack"),
        )
        .expect("link");

        let mut spec = ResolvedBuildSpec::new("stats");
        spec.workspace.root_dir = workspace.display().to_string();
        // Keep the real user cache out of the test.
        let buildroot = &mut spec.policy.providers.buildroot;
        buildroot.ccache.dir = Some(scratch.join("ccache").display().to_string());
        buildroot.package_cache.system_dir = Some(scratch.join("packages").display().to_string());
        let report = cache_stats_in(&spec, Some(&user));
        let caches = report.json["caches"].as_array().expect("caches");
        let cache = |name: &str| {
            caches
                .iter()
                .find(|cache| cache["name"] == name)
                .unwrap_or_else(|| panic!("no cache {name}: {caches:?}"))
        };
        assert_eq!(report.json["action"], "stats");
        assert_eq!(cache("git-mirrors")["exists"], true);
        assert_eq!(cache("git-mirrors")["files"], 1);
        assert_eq!(
            cache("git-mirrors")["apparent_bytes"].as_u64(),
            Some(8192 + dir_len(&workspace.join(".gaia/cache/git")))
        );
        assert_eq!(cache("buildroot-dl")["files"], 1);
        assert_eq!(cache("cargo-target/abc123")["files"], 1);
        assert_eq!(cache("downloads")["exists"], false);
        assert_eq!(cache("docker-home")["exists"], true);
        for name in [
            "import-sources",
            "ccache",
            "docker-cache",
            "buildroot-packages (system)",
        ] {
            cache(name);
        }
        assert_eq!(report.json["total"]["files"], 2);
        // The user-wide defaults are listed although this build uses others.
        cache("buildroot-packages (user)");
        assert_eq!(
            cache("ccache (user)")["path"],
            user.join("buildroot/ccache").display().to_string()
        );
        assert!(
            report.lines.last().expect("total").starts_with("total: "),
            "{:?}",
            report.lines
        );
        assert!(
            report
                .lines
                .iter()
                .any(|line| line.starts_with("downloads") && line.ends_with("(not present)")),
            "{:?}",
            report.lines
        );
    }

    /// The apparent size of the directories of a tree.
    fn dir_len(root: &Path) -> u64 {
        let mut total = 0;
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            total += fs::symlink_metadata(&dir).map_or(0, |meta| meta.len());
            for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    stack.push(entry.path());
                }
            }
        }
        total
    }
}
