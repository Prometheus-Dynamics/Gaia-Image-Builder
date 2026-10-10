//! Explanations of reuse decisions: what changed for an operation to run, and
//! which operations run directly versus only because a dependency runs.

use crate::{ExecutionPlan, OperationId, OperationKind, OperationReuse, PlannedOperation};
use gaia_spec::{ResolvedBuildSpec, SourceDefinition};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Reason code of an operation that runs only because a dependency runs.
pub(crate) const DEPENDENCY_REBUILT: &str = "dependency_rebuilt";

/// The operations a run would execute, split by cause.
///
/// `direct` operations have an input of their own that changed (a spec or
/// source pin, a changed output, a missing output, or a new operation).
/// `cascaded` operations run only because a dependency runs. Build
/// resolution and report emission always run and are not counted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InvalidationSummary {
    pub direct: Vec<String>,
    pub cascaded: Vec<String>,
}

pub fn invalidation_summary(plan: &ExecutionPlan) -> InvalidationSummary {
    let mut summary = InvalidationSummary::default();
    for operation in &plan.operations {
        let OperationReuse::Execute(reason) = &operation.reuse else {
            continue;
        };
        // Resolution and reporting run whatever the state says; their reason
        // code can still read as a missing state entry, so match the kind.
        if matches!(
            operation.kind,
            OperationKind::ResolveBuild | OperationKind::EmitReport
        ) {
            continue;
        }
        let id = operation.id.as_str().to_string();
        if reason.code == DEPENDENCY_REBUILT {
            summary.cascaded.push(id);
        } else {
            summary.direct.push(id);
        }
    }
    summary
}

/// The dependencies (build resolution excluded) that execute in this plan,
/// sorted. `decisions` maps each operation already decided to whether it
/// reuses (true) or executes (false).
pub(crate) fn rebuilding_dependencies(
    operation: &PlannedOperation,
    decisions: &HashMap<String, bool>,
) -> Vec<String> {
    let mut names = operation
        .depends_on
        .iter()
        .filter(|dependency| dependency.as_str() != OperationId::resolve().as_str())
        .filter(|dependency| !decisions.get(dependency.as_str()).copied().unwrap_or(false))
        .map(|dependency| dependency.as_str().to_string())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

/// How many rebuilding dependencies a cascade message names before counting
/// the rest; a build can depend on dozens of operations.
const LISTED_DEPENDENCIES: usize = 3;

/// The clause of a cascade message naming the dependencies that run. A single
/// dependency also gives its own reason code, which says why it runs.
pub(crate) fn describe_rebuilding(
    names: &[String],
    codes: &HashMap<String, &'static str>,
) -> String {
    match names {
        [] => "a dependency runs".to_string(),
        [one] => match codes.get(one) {
            Some(code) => format!("{one} runs ({code})"),
            None => format!("{one} runs"),
        },
        _ if names.len() <= LISTED_DEPENDENCIES => format!("{} run", names.join(", ")),
        _ => format!(
            "{} and {} more run",
            names[..LISTED_DEPENDENCIES].join(", "),
            names.len() - LISTED_DEPENDENCIES
        ),
    }
}

/// Why an operation's fingerprint no longer matches. A git source whose
/// locked commit moved names the rev change; other fingerprint changes record
/// no detail (state written before per-input records existed).
pub(crate) fn fingerprint_change_message(
    spec: &ResolvedBuildSpec,
    id: &OperationId,
    kind: &OperationKind,
) -> String {
    match source_rev_change(spec, kind) {
        Some((old, new)) => format!(
            "operation '{}' will execute because its rev changed: {} -> {}",
            id.as_str(),
            short_rev(&old),
            short_rev(&new)
        ),
        None => format!(
            "operation '{}' will execute because its fingerprint changed (no detail recorded)",
            id.as_str()
        ),
    }
}

fn short_rev(rev: &str) -> &str {
    rev.get(..7).unwrap_or(rev)
}

/// The commit a git source last materialized and its locked commit, when
/// both are known and differ. The old commit comes from the source state the
/// last materialization wrote; the new one from the spec's lockfile entry.
fn source_rev_change(spec: &ResolvedBuildSpec, kind: &OperationKind) -> Option<(String, String)> {
    let OperationKind::MaterializeSource { source_id } = kind else {
        return None;
    };
    let source = spec.sources.iter().find(|source| source.id == *source_id)?;
    let SourceDefinition::Git(git) = &source.definition else {
        return None;
    };
    let new = git.locked_commit.as_deref()?.trim();
    let state = source_state(spec, source_id.as_str())?;
    let old = ["resolved_commit_sha", "materialized_head_commit"]
        .iter()
        .find_map(|key| state.get(*key))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;
    (old != new).then(|| (old, new.to_string()))
}

/// The tree digest a git source recorded when it last materialized, as the
/// content its dependents consume. The digest covers the checked-out files
/// and not the commit, so a rev bump with identical files keeps it, and a
/// real content change moves it. Sources without a recorded digest keep the
/// whole state file (`None`).
pub(crate) fn source_tree_content_signature(
    spec: &ResolvedBuildSpec,
    source_id: &str,
) -> Option<String> {
    let state = source_state(spec, source_id)?;
    state
        .get("materialized_tree_digest")
        .map(|digest| digest.trim())
        .filter(|digest| !digest.is_empty())
        .map(|digest| format!("tree:{digest}"))
}

fn source_state(
    spec: &ResolvedBuildSpec,
    source_id: &str,
) -> Option<std::collections::BTreeMap<String, String>> {
    let path: PathBuf = crate::reuse::resolve_workspace_path(spec, &spec.workspace.build_dir)
        .join("sources")
        .join(source_id)
        .join(".gaia-source-state.txt");
    let contents = fs::read_to_string(path).ok()?;
    Some(gaia_spec::KeyValueState::parse(&contents).into_map())
}

#[cfg(test)]
mod tests {
    use super::describe_rebuilding;
    use std::collections::HashMap;

    #[test]
    fn describe_rebuilding_names_a_single_cause_and_truncates_long_lists() {
        let codes: HashMap<String, &'static str> =
            [("image:prepare".to_string(), "operation_output_changed")].into();
        assert_eq!(
            describe_rebuilding(&["image:prepare".to_string()], &codes),
            "image:prepare runs (operation_output_changed)"
        );
        assert_eq!(
            describe_rebuilding(&["a".to_string(), "b".to_string()], &codes),
            "a, b run"
        );
        let many = (0..5).map(|index| format!("op{index}")).collect::<Vec<_>>();
        assert_eq!(
            describe_rebuilding(&many, &codes),
            "op0, op1, op2 and 2 more run"
        );
    }
}
