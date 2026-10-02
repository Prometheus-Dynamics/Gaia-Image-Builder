use gaia_config::lockfile::{GitLockEntry, GitLockStatus, GitLockfile, lockfile_path};
use gaia_config::{ResolveOptions, try_resolve_config_with_options};
use gaia_spec::{ResolvedBuildSpec, SourceDefinition};
use std::path::PathBuf;
use std::time::Duration;

use crate::LockArgs;

use super::CommandOutcome;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockReport {
    pub lockfile: PathBuf,
    pub entries: Vec<LockReportEntry>,
    /// Source ids whose entries were dropped because the build no longer
    /// has such a git source.
    pub removed: Vec<String>,
    /// Git sources pinned with `rev`, which need no lock entry.
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockReportEntry {
    pub source: String,
    pub reference: String,
    pub commit: String,
    pub change: LockChange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockChange {
    Added,
    Updated { previous: String },
    Unchanged,
}

pub fn lock_build_command(
    build: &str,
    options: &ResolveOptions,
    lock_args: &LockArgs,
) -> CommandOutcome {
    // Import sources without `rev` need a lock entry to resolve; locking is
    // what creates that entry, so let resolution follow their current ref.
    let options = ResolveOptions {
        resolve_unpinned_import_sources: true,
        ..options.clone()
    };
    let spec = match try_resolve_config_with_options(build, &options) {
        Ok(spec) => spec,
        Err(error) => {
            return CommandOutcome::Failed {
                message: error.to_string(),
            };
        }
    };
    let timeout = Duration::from_secs(spec.policy.providers.git.timeout_seconds.max(1));
    match lock_build(&spec, lock_args, |git| {
        gaia_source_providers::resolve_git_source_commit(git, timeout)
    }) {
        Ok(report) => CommandOutcome::Locked { spec, report },
        Err(message) => CommandOutcome::Failed { message },
    }
}

fn lock_build(
    spec: &ResolvedBuildSpec,
    lock_args: &LockArgs,
    resolve: impl Fn(&gaia_spec::GitSourceSpec) -> Result<String, String>,
) -> Result<LockReport, String> {
    let path = lockfile_path(spec).ok_or_else(|| {
        format!(
            "build '{}' was not loaded from a file, so it has no lockfile location",
            spec.identity.display_name
        )
    })?;
    let git_sources = spec
        .sources
        .iter()
        .filter_map(|source| match &source.definition {
            SourceDefinition::Git(git) => Some((source.id.as_str(), git)),
            _ => None,
        })
        .collect::<Vec<_>>();
    for requested in &lock_args.sources {
        if !git_sources.iter().any(|(id, _)| id == requested) {
            return Err(format!(
                "--update names '{requested}', which is not a git source of build '{}'",
                spec.identity.display_name
            ));
        }
    }
    let regenerate_all = lock_args.update && lock_args.sources.is_empty();
    let existing = match GitLockfile::load(&path) {
        Ok(lock) => lock.unwrap_or_default(),
        // A full update rewrites the file, so a broken one can be replaced.
        Err(_) if regenerate_all => GitLockfile::default(),
        Err(error) => return Err(format!("{error}; run `gaia lock <build> --update`")),
    };

    let mut next = GitLockfile::default();
    let mut entries = Vec::new();
    let mut skipped = Vec::new();
    for (id, git) in &git_sources {
        if git.rev.is_some() {
            skipped.push((*id).to_string());
            continue;
        }
        let refresh = lock_args.update
            && (lock_args.sources.is_empty() || lock_args.sources.iter().any(|name| name == id));
        let previous = existing.entry_for(id);
        let entry = match existing.status(id, git) {
            GitLockStatus::Locked(entry) if !refresh => entry.clone(),
            _ => {
                let commit = resolve(git).map_err(|error| {
                    format!(
                        "failed to resolve git source '{id}' ({}): {error}",
                        git.repo
                    )
                })?;
                GitLockEntry::new(id, git, commit)
            }
        };
        let change = match previous {
            None => LockChange::Added,
            Some(previous) if previous == &entry => LockChange::Unchanged,
            Some(previous) => LockChange::Updated {
                previous: format!("{} {}", previous.reference, previous.commit),
            },
        };
        entries.push(LockReportEntry {
            source: entry.source.clone(),
            reference: entry.reference.clone(),
            commit: entry.commit.clone(),
            change,
        });
        next.git.push(entry);
    }
    let removed = existing
        .git
        .iter()
        .filter(|entry| next.entry_for(&entry.source).is_none())
        .map(|entry| entry.source.clone())
        .collect();
    next.write(&path)?;
    Ok(LockReport {
        lockfile: path,
        entries,
        removed,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaia_spec::{GitSourceSpec, SourcePinPolicySpec, SourceRefreshPolicySpec, SourceSpec};
    use std::cell::Cell;
    use std::fs;

    fn spec_with_sources(dir: &std::path::Path) -> ResolvedBuildSpec {
        let mut spec = ResolvedBuildSpec::new("lock-test");
        spec.selection.selected_build_file = Some(dir.join("cm5.toml").display().to_string());
        for (id, branch) in [("orion", "main"), ("tools", "dev")] {
            spec.sources.push(SourceSpec::new(
                id,
                SourceDefinition::Git(GitSourceSpec {
                    repo: format!("https://example.invalid/{id}.git"),
                    branch: Some(branch.into()),
                    tag: None,
                    rev: None,
                    subdir: None,
                    update: false,
                    refresh_policy: SourceRefreshPolicySpec::Auto,
                    pin_policy: SourcePinPolicySpec::Locked,
                    locked_commit: None,
                }),
            ));
        }
        spec
    }

    #[test]
    fn lock_keeps_existing_entries_and_updates_only_named_sources() {
        let dir =
            std::env::temp_dir().join(format!("gaia-app-lock-{}-{}", std::process::id(), line!()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        let spec = spec_with_sources(&dir);
        let round = Cell::new(0);
        let resolver = |git: &GitSourceSpec| -> Result<String, String> {
            Ok(format!("{}-{}", git.branch.clone().unwrap(), round.get()))
        };

        let report = lock_build(&spec, &LockArgs::default(), resolver).expect("lock");
        assert!(
            report
                .entries
                .iter()
                .all(|entry| entry.change == LockChange::Added)
        );
        assert_eq!(report.lockfile, dir.join("cm5.gaia.lock"));

        round.set(1);
        let report = lock_build(&spec, &LockArgs::default(), resolver).expect("relock");
        assert!(
            report
                .entries
                .iter()
                .all(|entry| entry.change == LockChange::Unchanged),
            "plain lock must not move existing entries"
        );

        let update_tools = LockArgs {
            update: true,
            sources: vec!["tools".into()],
        };
        let report = lock_build(&spec, &update_tools, resolver).expect("update");
        let by_id = |id: &str| {
            report
                .entries
                .iter()
                .find(|entry| entry.source == id)
                .unwrap()
                .clone()
        };
        assert_eq!(by_id("orion").commit, "main-0");
        assert_eq!(by_id("tools").commit, "dev-1");
        assert!(matches!(by_id("tools").change, LockChange::Updated { .. }));

        let unknown = LockArgs {
            update: true,
            sources: vec!["missing".into()],
        };
        assert!(lock_build(&spec, &unknown, resolver).is_err());

        // Removing a source drops its entry.
        let mut fewer = spec.clone();
        fewer.sources.retain(|source| source.id.as_str() == "orion");
        let report = lock_build(&fewer, &LockArgs::default(), resolver).expect("prune");
        assert_eq!(report.removed, vec!["tools".to_string()]);
        let lock = GitLockfile::load(&dir.join("cm5.gaia.lock"))
            .expect("load")
            .expect("present");
        assert_eq!(lock.git.len(), 1);
        let _ = fs::remove_dir_all(dir);
    }
}
