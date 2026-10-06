//! Rust build groups: every member is built by the same cargo invocation,
//! so everything that shapes that invocation must agree across members.

use std::collections::BTreeMap;

use gaia_spec::{ArtifactDefinition, ArtifactSpec, ResolvedBuildSpec, RustArtifactSpec};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;

pub(crate) fn validate_build_groups(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let mut groups = BTreeMap::<&str, Vec<(&ArtifactSpec, &RustArtifactSpec)>>::new();
    for artifact in &spec.artifacts {
        let ArtifactDefinition::Rust(rust) = &artifact.definition else {
            continue;
        };
        let Some(group) = rust.build_group.as_deref() else {
            continue;
        };
        if group.trim().is_empty() {
            diagnostics.push(error(
                "rust_build_group_empty",
                format!(
                    "rust artifact '{}' has an empty build_group",
                    artifact.id.as_str()
                ),
                Some(format!("artifact:{}", artifact.id.as_str())),
            ));
            continue;
        }
        groups.entry(group).or_default().push((artifact, rust));
    }
    for (group, members) in groups {
        let Some(((leader, leader_rust), rest)) = members.split_first() else {
            continue;
        };
        for (member, member_rust) in rest {
            let conflicts = [
                (
                    "source",
                    format!("{:?}", leader.source.as_ref().map(|source| &source.id)),
                    format!("{:?}", member.source.as_ref().map(|source| &source.id)),
                ),
                (
                    "target",
                    format!("{:?}", leader.target),
                    format!("{:?}", member.target),
                ),
                (
                    "profile",
                    format!("{:?}", leader.build_mode),
                    format!("{:?}", member.build_mode),
                ),
                (
                    "execution",
                    format!("{:?}", leader.execution),
                    format!("{:?}", member.execution),
                ),
                (
                    "no_default_features",
                    leader_rust.no_default_features.to_string(),
                    member_rust.no_default_features.to_string(),
                ),
                (
                    "all_features",
                    leader_rust.all_features.to_string(),
                    member_rust.all_features.to_string(),
                ),
            ];
            for (field, leader_value, member_value) in conflicts {
                if leader_value != member_value {
                    diagnostics.push(error(
                        "rust_build_group_conflict",
                        format!(
                            "rust build group '{group}': artifacts '{}' and '{}' must share \
                             {field} to build in one cargo invocation ({leader_value} vs \
                             {member_value})",
                            leader.id.as_str(),
                            member.id.as_str()
                        ),
                        Some(format!("artifact:{}", member.id.as_str())),
                    ));
                }
            }
        }
    }
}
