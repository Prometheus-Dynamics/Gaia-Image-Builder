//! `--rebuild` and `--rebuild-package`: operations a run executes regardless
//! of reuse state. Their dependents follow the normal `dependency_rebuilt`
//! logic, so content-based reuse can still reuse them.

use crate::{ExecutionPlan, OperationKind, PlanTarget};
use gaia_spec::wildcard_match;

/// Reason code of an operation that executes because of a rebuild request.
pub const REBUILD_REQUESTED: &str = "rebuild_requested";

/// What a run was asked to rebuild, from the command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RebuildRequest {
    /// `--rebuild` targets: domains, operation ids, or ids with `*` globs.
    pub targets: Vec<PlanTarget>,
    /// `--rebuild-package` names: Buildroot packages that are dircleaned
    /// before make and not restored from the package cache.
    pub packages: Vec<String>,
}

impl RebuildRequest {
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty() && self.packages.is_empty()
    }

    /// Why the operation must execute because of this request, or `None`.
    /// Report and resolve operations are never forced.
    pub fn reason(&self, id: &str, kind: &OperationKind) -> Option<String> {
        if matches!(
            kind,
            OperationKind::ResolveBuild | OperationKind::EmitReport
        ) {
            return None;
        }
        let named = self.targets.iter().any(|target| match target {
            PlanTarget::Domain(domain) => domain.contains(kind),
            PlanTarget::Operation(pattern) => wildcard_match(pattern, id),
        });
        if named {
            return Some(format!(
                "operation '{id}' will execute because --rebuild names it"
            ));
        }
        if !self.packages.is_empty()
            && matches!(
                kind,
                OperationKind::PrepareImage | OperationKind::BuildImage
            )
        {
            return Some(format!(
                "operation '{id}' will execute because --rebuild-package names packages it builds: {}",
                self.packages.join(", ")
            ));
        }
        None
    }

    /// Sets the reason of every forced operation. Used when there is no
    /// reuse state: everything already executes, only the reason changes.
    pub(crate) fn mark(&self, plan: &mut ExecutionPlan) {
        for operation in &mut plan.operations {
            if let Some(message) = self.reason(operation.id.as_str(), &operation.kind) {
                operation.reuse = crate::OperationReuse::execute(REBUILD_REQUESTED, message);
            }
        }
    }

    /// Errors when an operation target names nothing in `plan` (the full
    /// plan, before `--only` narrows it), listing the close ids.
    pub fn check(&self, plan: &ExecutionPlan) -> Result<(), String> {
        for target in &self.targets {
            let PlanTarget::Operation(pattern) = target else {
                continue;
            };
            let ids = plan
                .operations
                .iter()
                .map(|operation| operation.id.as_str())
                .collect::<Vec<_>>();
            if ids.iter().any(|id| wildcard_match(pattern, id)) {
                continue;
            }
            return Err(if pattern.contains('*') {
                format!("--rebuild '{pattern}' matches no operation in the plan")
            } else {
                format!(
                    "--rebuild names unknown operation '{pattern}'{}",
                    close_matches_hint(pattern, &ids)
                )
            });
        }
        Ok(())
    }
}

/// `" (did you mean 'a', 'b'?)"` for the closest ids, or nothing.
fn close_matches_hint(wanted: &str, ids: &[&str]) -> String {
    let limit = (wanted.chars().count() / 3).max(3);
    let mut scored = ids
        .iter()
        .filter_map(|id| {
            let distance = edit_distance(wanted, id);
            (distance <= limit || id.contains(wanted)).then_some((distance, *id))
        })
        .collect::<Vec<_>>();
    scored.sort();
    scored.truncate(3);
    if scored.is_empty() {
        return String::new();
    }
    let names = scored
        .iter()
        .map(|(_, id)| format!("'{id}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(" (did you mean {names}?)")
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b = b.chars().collect::<Vec<_>>();
    let mut previous = (0..=b.len()).collect::<Vec<_>>();
    for (i, ca) in a.chars().enumerate() {
        let mut current = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != *cb);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        previous = current;
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_matches_suggest_the_nearest_ids() {
        let ids = ["artifact:photonvision-jar", "image:build", "image:prepare"];
        let hint = close_matches_hint("artifact:photonvison-jar", &ids);
        assert_eq!(hint, " (did you mean 'artifact:photonvision-jar'?)");
        assert_eq!(close_matches_hint("zzzzzzzzzzzz", &ids), "");
    }

    #[test]
    fn edit_distance_counts_single_edits() {
        assert_eq!(edit_distance("image", "image"), 0);
        assert_eq!(edit_distance("image", "imagr"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
    }
}
