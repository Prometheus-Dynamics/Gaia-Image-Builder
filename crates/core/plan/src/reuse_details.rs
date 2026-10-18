//! Named input components of an operation, so a reuse decision that rests on
//! a changed fingerprint can say which input changed.
//!
//! The components are the operation's fingerprint inputs
//! (`reuse_inputs::operation_inputs`, the same list the fingerprint hashes),
//! each with its own digest, plus the content signature of each dependency
//! (what the input signature hashes). Map-like inputs (Buildroot
//! `config_overrides`, a Java artifact's `build_env`) also have one part per
//! key, named `<map>[<key>]`, so the keys that changed can be named. The parts
//! only explain a decision: reuse is still decided by the fingerprint.

use crate::reuse::operation_content_signature;
use crate::reuse_inputs::operation_inputs;
use crate::{ExecutionPlan, OperationKind, PlannedOperation, operation_fingerprint};
use gaia_spec::ResolvedBuildSpec;
use gaia_spec::fingerprint::sha256_hex;
use std::collections::BTreeMap;

/// The named inputs of `operation`, as `(name, digest)` pairs in a stable
/// order. Digests are SHA-256 hex of each input's canonical text, so they
/// are safe to store on one line. Dependencies are listed by their content
/// signature.
pub fn operation_components(
    spec: &ResolvedBuildSpec,
    plan: &ExecutionPlan,
    operation: &PlannedOperation,
) -> Vec<(String, String)> {
    let mut parts = operation_inputs(spec, &operation.kind);
    // Image operations name their inputs "input <op>", the others "dependency
    // <op>". Build resolution is excluded, as in the input signature.
    let label = if matches!(
        operation.kind,
        OperationKind::PrepareImage | OperationKind::BuildImage | OperationKind::AssembleImage
    ) {
        "input"
    } else {
        "dependency"
    };
    let mut dependencies = operation
        .depends_on
        .iter()
        .filter(|dependency| dependency.as_str() != crate::OperationId::resolve().as_str())
        .map(|dependency| dependency.as_str().to_string())
        .collect::<Vec<_>>();
    dependencies.sort();
    dependencies.dedup();
    for dependency in dependencies {
        let content = plan
            .operations
            .iter()
            .find(|candidate| candidate.id.as_str() == dependency)
            .and_then(|candidate| operation_content_signature(spec, &candidate.kind));
        parts.push((
            format!("{label} {dependency}"),
            content.unwrap_or_else(|| "none".to_string()),
        ));
    }
    parts
        .into_iter()
        .map(|(name, text)| (name, sha256_hex(&text)))
        .collect()
}

/// The fingerprint an operation is recorded under when a run finishes.
///
/// An operation's fingerprint is taken when the plan is made, before the run.
/// Image assembly hashes its build's outputs and the Buildroot files it
/// reads, and the build rewrites those during the run. Its components and
/// input signature are recorded after the run, so its fingerprint is taken
/// then too: otherwise the next plan compares a fingerprint of the old outputs
/// with the new ones and reruns an assembly nothing changed.
pub fn recorded_fingerprint(spec: &ResolvedBuildSpec, operation: &PlannedOperation) -> u64 {
    match operation.kind {
        OperationKind::AssembleImage => operation_fingerprint(spec, &operation.kind),
        _ => operation.fingerprint,
    }
}

/// The part name without its `[key]` suffix.
fn base_name(name: &str) -> &str {
    name.split_once('[').map_or(name, |(base, _)| base)
}

/// The key of a `base[key]` part, when `name` is one of `base`'s.
fn key_of<'name>(name: &'name str, base: &str) -> Option<&'name str> {
    name.strip_prefix(base)?
        .strip_prefix('[')?
        .strip_suffix(']')
}

/// The message for an operation whose fingerprint changed, naming the inputs
/// whose digests differ between the recorded and the current components,
/// such as `config_overrides changed (BR2_A, BR2_B)`. `None` when every named
/// input matches, so the change is in an input that is not split out.
pub fn fingerprint_change_detail(
    id: &str,
    recorded: &[(String, String)],
    current: &[(String, String)],
) -> Option<String> {
    let recorded_map: BTreeMap<&str, &str> = recorded
        .iter()
        .map(|(name, digest)| (name.as_str(), digest.as_str()))
        .collect();
    let current_map: BTreeMap<&str, &str> = current
        .iter()
        .map(|(name, digest)| (name.as_str(), digest.as_str()))
        .collect();
    let names = || {
        current
            .iter()
            .map(|(name, _)| name.as_str())
            .chain(recorded.iter().map(|(name, _)| name.as_str()))
    };
    let mut bases = Vec::<&str>::new();
    for name in names() {
        let base = base_name(name);
        if !bases.contains(&base) {
            bases.push(base);
        }
    }

    let mut phrases = Vec::new();
    for base in bases {
        // A component the recorded state never named (a kind of input a newer
        // Gaia started to record) is new, not changed: its earlier value is
        // unknown.
        let recorded_before = recorded_map.contains_key(base)
            || recorded_map.keys().any(|name| key_of(name, base).is_some());
        if !recorded_before {
            phrases.push(format!(
                "{base} has no earlier record (first run with this Gaia)"
            ));
            continue;
        }
        // An input the recorded state had and this operation no longer has.
        let current_before = current_map.contains_key(base)
            || current_map.keys().any(|name| key_of(name, base).is_some());
        if !current_before {
            phrases.push(format!("{base} is no longer an input"));
            continue;
        }
        let whole_changed = recorded_map.get(base) != current_map.get(base);
        let mut keys = Vec::<&str>::new();
        for name in names() {
            let Some(key) = key_of(name, base) else {
                continue;
            };
            if !keys.contains(&key) && recorded_map.get(name) != current_map.get(name) {
                keys.push(key);
            }
        }
        if !whole_changed && keys.is_empty() {
            continue;
        }
        keys.sort_unstable();
        phrases.push(if keys.is_empty() {
            format!("{base} changed")
        } else {
            format!("{base} changed ({})", keys.join(", "))
        });
    }
    (!phrases.is_empty()).then(|| {
        format!(
            "operation '{id}' will execute because {}",
            phrases.join("; ")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(entries: &[(&str, &str)]) -> Vec<(String, String)> {
        entries
            .iter()
            .map(|(name, digest)| (name.to_string(), digest.to_string()))
            .collect()
    }

    #[test]
    fn a_changed_override_key_is_named() {
        let recorded = parts(&[
            ("defconfig", "a"),
            ("config_overrides", "old"),
            ("config_overrides[BR2_A]", "1"),
            ("config_overrides[BR2_B]", "2"),
        ]);
        let current = parts(&[
            ("defconfig", "a"),
            ("config_overrides", "new"),
            ("config_overrides[BR2_A]", "1"),
            ("config_overrides[BR2_B]", "3"),
        ]);
        assert_eq!(
            fingerprint_change_detail("image", &recorded, &current).as_deref(),
            Some("operation 'image' will execute because config_overrides changed (BR2_B)")
        );
    }

    #[test]
    fn added_and_removed_keys_and_whole_input_changes_are_named() {
        let recorded = parts(&[
            ("fragments", "a"),
            ("build_env", "x"),
            ("build_env[K]", "1"),
        ]);
        let current = parts(&[
            ("fragments", "b"),
            ("build_env", "y"),
            ("build_env[L]", "1"),
        ]);
        assert_eq!(
            fingerprint_change_detail("artifact:app", &recorded, &current).as_deref(),
            Some(
                "operation 'artifact:app' will execute because fragments changed; build_env changed (K, L)"
            )
        );
    }

    #[test]
    fn a_component_with_no_earlier_record_is_not_called_changed() {
        let recorded = parts(&[("defconfig", "a")]);
        let current = parts(&[
            ("defconfig", "a"),
            ("build command script myboard/build.sh", "s"),
            ("external file tree/x", "f"),
        ]);
        assert_eq!(
            fingerprint_change_detail("image", &recorded, &current).as_deref(),
            Some(
                "operation 'image' will execute because build command script myboard/build.sh has no earlier record (first run with this Gaia); external file tree/x has no earlier record (first run with this Gaia)"
            )
        );
    }

    #[test]
    fn a_recorded_component_that_is_gone_is_not_called_changed() {
        let recorded = parts(&[("defconfig", "a"), ("post-image script x.sh", "s")]);
        let current = parts(&[("defconfig", "a")]);
        assert_eq!(
            fingerprint_change_detail("image:prepare", &recorded, &current).as_deref(),
            Some(
                "operation 'image:prepare' will execute because post-image script x.sh is no longer an input"
            )
        );
    }

    #[test]
    fn a_recorded_component_that_changed_is_still_changed() {
        let recorded = parts(&[("post-image", "old")]);
        let current = parts(&[("post-image", "new")]);
        assert_eq!(
            fingerprint_change_detail("image", &recorded, &current).as_deref(),
            Some("operation 'image' will execute because post-image changed")
        );
    }

    #[test]
    fn identical_components_name_nothing() {
        let same = parts(&[("defconfig", "a"), ("build_env[K]", "1")]);
        assert_eq!(fingerprint_change_detail("image", &same, &same), None);
    }
}
