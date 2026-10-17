//! Plan-time resolution of floating git refs.
//!
//! A git source that tracks a branch or `HEAD` and refreshes automatically
//! names no commit in the spec, so planning used to re-materialize every
//! remote one. Here the ref is resolved with `git ls-remote` (one call per
//! distinct ref per plan, run in parallel) and compared with the commit the
//! last materialization recorded. An unchanged commit keeps the checkout and
//! what was built from it. A moved commit is part of the source's fingerprint,
//! so it rebuilds.

use crate::reuse::local_repo_path;
use crate::reuse_explain::source_state;
use gaia_spec::{
    GitBackendSpec, GitSourceSpec, ResolvedBuildSpec, SourceDefinition, SourcePinPolicySpec,
    SourceRefreshPolicySpec,
};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Bounds each plan-time `git ls-remote`. A slower remote falls back to
/// materializing the source, which is what planning did before this check.
pub(crate) const FLOATING_REF_TIMEOUT: Duration = Duration::from_secs(30);

/// The commit a ref resolves to, or the reason it did not resolve.
type Resolution = Result<String, String>;

/// Resolutions made for the plan in progress, keyed by [`cache_key`]. Filled
/// by [`FloatingRefScope::begin`] and cleared when that scope ends, so a later
/// plan never reads a commit from an earlier one.
fn resolutions() -> &'static Mutex<HashMap<String, Resolution>> {
    static RESOLUTIONS: OnceLock<Mutex<HashMap<String, Resolution>>> = OnceLock::new();
    RESOLUTIONS.get_or_init(Default::default)
}

fn cache_key(git: &GitSourceSpec) -> String {
    format!(
        "{}\u{1f}{}",
        git.repo,
        git.branch.as_deref().unwrap_or("HEAD")
    )
}

/// Whether plan-time resolution applies to `git`: a branch or `HEAD` that is
/// refreshed automatically and floats. A remote repository also needs the
/// policy that allows git to contact it; a local one never needs the network.
/// A source whose git runs on the docker backend is not resolved from the
/// plan, so it keeps the behaviour it had.
pub(crate) fn applies(spec: &ResolvedBuildSpec, git: &GitSourceSpec) -> bool {
    git.refresh_policy == SourceRefreshPolicySpec::Auto
        && git.pin_policy == SourcePinPolicySpec::Floating
        && git.locked_commit.is_none()
        && git.rev.is_none()
        && git.tag.is_none()
        && (local_repo_path(&git.repo).is_some()
            || spec.policy.providers.git.allow_remote_resolution)
        && (git.git_backend != GitBackendSpec::Execution || spec.policy.execution.docker.is_none())
}

/// The commit `git`'s floating ref points at now. Served from the current
/// plan's resolutions when there are any; otherwise resolved directly.
pub(crate) fn resolved_commit(git: &GitSourceSpec) -> Resolution {
    let key = cache_key(git);
    if let Some(found) = resolutions()
        .lock()
        .ok()
        .and_then(|map| map.get(&key).cloned())
    {
        return found;
    }
    gaia_source_providers::resolve_git_source_commit(git, FLOATING_REF_TIMEOUT)
}

/// Resolves the floating refs of one plan. Dropping it forgets them.
pub(crate) struct FloatingRefScope;

impl FloatingRefScope {
    /// Resolves each distinct applicable ref of `spec` once, in parallel.
    pub(crate) fn begin(spec: &ResolvedBuildSpec) -> Self {
        let mut distinct = HashMap::<String, &GitSourceSpec>::new();
        for source in &spec.sources {
            if let SourceDefinition::Git(git) = &source.definition
                && applies(spec, git)
            {
                distinct.entry(cache_key(git)).or_insert(git);
            }
        }
        let resolved = std::thread::scope(|scope| {
            let handles = distinct
                .into_iter()
                .map(|(key, git)| {
                    scope.spawn(move || {
                        let commit = gaia_source_providers::resolve_git_source_commit(
                            git,
                            FLOATING_REF_TIMEOUT,
                        );
                        (key, commit)
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .filter_map(|handle| handle.join().ok())
                .collect::<Vec<_>>()
        });
        if let Ok(mut map) = resolutions().lock() {
            map.clear();
            map.extend(resolved);
        }
        Self
    }
}

impl Drop for FloatingRefScope {
    fn drop(&mut self) {
        if let Ok(mut map) = resolutions().lock() {
            map.clear();
        }
    }
}

/// What a floating ref means for one git source.
pub(crate) enum FloatingCheck {
    /// Plan-time resolution does not apply to this source.
    NotApplicable,
    /// The ref still points at the commit the last materialization recorded.
    Unchanged { note: String },
    /// The ref moved, or no commit was recorded for it.
    Moved {
        label: String,
        from: Option<String>,
        to: String,
    },
    /// The ref could not be resolved, so the source materializes.
    Unresolved { label: String, error: String },
}

pub(crate) fn check(
    spec: &ResolvedBuildSpec,
    source_id: &str,
    git: &GitSourceSpec,
) -> FloatingCheck {
    if !applies(spec, git) {
        return FloatingCheck::NotApplicable;
    }
    let label = format!("floating ref '{}'", git.branch.as_deref().unwrap_or("HEAD"));
    match resolved_commit(git) {
        Err(error) => FloatingCheck::Unresolved { label, error },
        Ok(commit) => match recorded_commit(spec, source_id) {
            Some(recorded) if recorded == commit => FloatingCheck::Unchanged {
                note: format!("{label} still at {}", short(&commit)),
            },
            from => FloatingCheck::Moved {
                label,
                from,
                to: commit,
            },
        },
    }
}

/// The reuse note for a materialization that the floating ref leaves
/// unchanged, so the plan can say why it was reused.
pub(crate) fn reuse_note(
    spec: &ResolvedBuildSpec,
    source_id: &str,
    git: &GitSourceSpec,
) -> Option<String> {
    match check(spec, source_id, git) {
        FloatingCheck::Unchanged { note } => Some(note),
        _ => None,
    }
}

/// The commit the last materialization of `source_id` recorded.
fn recorded_commit(spec: &ResolvedBuildSpec, source_id: &str) -> Option<String> {
    let state = source_state(spec, source_id)?;
    state
        .get("resolved_commit_sha")
        .map(|value| value.trim().to_string())
        .filter(|value| value.len() >= 7 && value.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The first seven characters of a commit, as the plan displays it.
pub(crate) fn short(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}
