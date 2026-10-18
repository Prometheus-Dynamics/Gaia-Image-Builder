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

    /// The request with names that stand for another operation of `plan`
    /// replaced by it (see [`image_prepare_alias`]).
    pub fn resolve_aliases(&self, plan: &ExecutionPlan) -> RebuildRequest {
        let mut resolved = self.clone();
        for target in &mut resolved.targets {
            if let PlanTarget::Operation(pattern) = target
                && let Some(alias) = image_prepare_alias(pattern, plan)
            {
                *pattern = alias.to_string();
            }
        }
        resolved
    }

    /// Errors when an operation target names nothing in `plan` (the full
    /// plan, before `--only` narrows it), listing the close ids.
    pub fn check(&self, plan: &ExecutionPlan) -> Result<(), String> {
        for target in &self.targets {
            let PlanTarget::Operation(pattern) = target else {
                continue;
            };
            if image_prepare_alias(pattern, plan).is_some() {
                continue;
            }
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

/// `image:build` for the name `image:prepare` when `plan` has no prepare
/// step: a build without one does its whole Buildroot make in `image:build`,
/// so forcing the name forces that make. `None` for any other name.
pub fn image_prepare_alias(pattern: &str, plan: &ExecutionPlan) -> Option<&'static str> {
    let prepare_planned = plan
        .operations
        .iter()
        .any(|operation| matches!(operation.kind, OperationKind::PrepareImage));
    let build_planned = plan
        .operations
        .iter()
        .any(|operation| matches!(operation.kind, OperationKind::BuildImage));
    (pattern == "image:prepare" && !prepare_planned && build_planned).then_some("image:build")
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
    use crate::{OperationId, PlannedOperation};

    fn plan_with(kinds: &[(&str, OperationKind)]) -> ExecutionPlan {
        ExecutionPlan {
            build_id: gaia_spec::ResolvedBuildSpec::new("rebuild-alias")
                .identity
                .id
                .clone(),
            operations: kinds
                .iter()
                .map(|(id, kind)| PlannedOperation::new(OperationId::new(*id), kind.clone()))
                .collect(),
        }
    }

    fn prepare_request() -> RebuildRequest {
        RebuildRequest {
            targets: vec![PlanTarget::Operation("image:prepare".into())],
            packages: Vec::new(),
        }
    }

    #[test]
    fn image_prepare_forces_the_image_build_when_no_prepare_step_is_planned() {
        let plan = plan_with(&[
            ("image:build", OperationKind::BuildImage),
            ("image:assembly", OperationKind::AssembleImage),
        ]);
        let request = prepare_request();
        request.check(&plan).expect("the name is accepted");
        let resolved = request.resolve_aliases(&plan);
        assert!(
            resolved
                .reason("image:build", &OperationKind::BuildImage)
                .is_some_and(|reason| reason.contains("--rebuild names it"))
        );
        assert!(
            resolved
                .reason("image:assembly", &OperationKind::AssembleImage)
                .is_none()
        );
    }

    #[test]
    fn image_prepare_keeps_its_meaning_when_a_prepare_step_is_planned() {
        let plan = plan_with(&[
            ("image:prepare", OperationKind::PrepareImage),
            ("image:build", OperationKind::BuildImage),
        ]);
        assert_eq!(image_prepare_alias("image:prepare", &plan), None);
        let resolved = prepare_request().resolve_aliases(&plan);
        assert!(
            resolved
                .reason("image:build", &OperationKind::BuildImage)
                .is_none()
        );
        assert!(
            resolved
                .reason("image:prepare", &OperationKind::PrepareImage)
                .is_some()
        );
    }

    #[test]
    fn image_prepare_is_still_unknown_without_any_image_step() {
        let plan = plan_with(&[(
            "artifact:app",
            OperationKind::BuildArtifact {
                artifact_id: "app".into(),
            },
        )]);
        let error = prepare_request().check(&plan).expect_err("unknown");
        assert!(
            error.contains("unknown operation 'image:prepare'"),
            "{error}"
        );
    }

    #[test]
    fn close_matches_suggest_the_nearest_ids() {
        let ids = ["artifact:myapp-jar", "image:build", "image:prepare"];
        let hint = close_matches_hint("artifact:myapp-jar", &ids);
        assert_eq!(hint, " (did you mean 'artifact:myapp-jar'?)");
        assert_eq!(close_matches_hint("zzzzzzzzzzzz", &ids), "");
    }

    #[test]
    fn edit_distance_counts_single_edits() {
        assert_eq!(edit_distance("image", "image"), 0);
        assert_eq!(edit_distance("image", "imagr"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
    }
}
