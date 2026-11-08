//! Rust build groups: every member is built by the same cargo invocation,
//! so everything that shapes that invocation must agree across members.

use std::collections::BTreeMap;

use gaia_spec::{
    ArtifactDefinition, ArtifactSpec, ResolvedBuildSpec, RustArtifactSpec, RustLinkSpec,
};

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
    for (group, mut members) in groups {
        validate_group_link(group, &members, diagnostics);
        // A crate-building member leads, so the standard library is compared
        // only on what its own build uses: source, target, profile, backend.
        members.sort_by_key(|(_, rust)| rust.is_std_library());
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
                (
                    "build_env",
                    format!("{:?}", leader_rust.build_env),
                    format!("{:?}", member_rust.build_env),
                ),
                (
                    "cpu",
                    format!("{:?}", leader_rust.cpu.as_ref().map(|cpu| &cpu.profile)),
                    format!("{:?}", member_rust.cpu.as_ref().map(|cpu| &cpu.profile)),
                ),
            ];
            let compared = if member_rust.is_std_library() || leader_rust.is_std_library() {
                4
            } else {
                conflicts.len()
            };
            for (field, leader_value, member_value) in conflicts.into_iter().take(compared) {
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

/// Every crate-building member links the same way, with the same LTO, since
/// one cargo run builds them all; the standard library (`package = "std"`)
/// only belongs in a group that links it dynamically.
fn validate_group_link(
    group: &str,
    members: &[(&ArtifactSpec, &RustArtifactSpec)],
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let (std_libraries, crates): (Vec<_>, Vec<_>) =
        members.iter().partition(|(_, rust)| rust.is_std_library());
    if let Some(((leader, leader_rust), rest)) = crates.split_first() {
        for (member, member_rust) in rest {
            if member_rust.link != leader_rust.link {
                diagnostics.push(error(
                    "rust_build_group_link_mismatch",
                    format!(
                        "rust build group '{group}': artifacts '{}' and '{}' must share link to \
                         build in one cargo invocation ({:?} vs {:?})",
                        leader.id.as_str(),
                        member.id.as_str(),
                        leader_rust.link,
                        member_rust.link
                    ),
                    Some(format!("artifact:{}", member.id.as_str())),
                ));
            }
            if member_rust.lto != leader_rust.lto {
                diagnostics.push(error(
                    "rust_build_group_lto_mismatch",
                    format!(
                        "rust build group '{group}': artifacts '{}' and '{}' must share lto to \
                         build in one cargo invocation ({:?} vs {:?})",
                        leader.id.as_str(),
                        member.id.as_str(),
                        leader_rust.lto,
                        member_rust.lto
                    ),
                    Some(format!("artifact:{}", member.id.as_str())),
                ));
            }
        }
    }
    let shared = crates
        .iter()
        .any(|(_, rust)| rust.link == RustLinkSpec::Shared);
    if !shared {
        for (artifact, _) in std_libraries {
            diagnostics.push(error(
                "rust_std_without_shared_link",
                format!(
                    "rust build group '{group}': artifact '{}' ships the standard library, \
                     but no artifact of the group sets link = \"shared\", so nothing links it",
                    artifact.id.as_str()
                ),
                Some(format!("artifact:{}", artifact.id.as_str())),
            ));
        }
    }
}
