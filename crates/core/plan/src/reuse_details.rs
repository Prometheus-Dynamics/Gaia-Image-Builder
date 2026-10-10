//! Named input components of an operation, so a reuse decision that rests on
//! a changed fingerprint can say which input changed.
//!
//! The operation fingerprint hashes an operation's inputs into one value.
//! [`operation_components`] lists the same inputs as named parts, each with
//! its own digest. Map-like inputs (Buildroot `config_overrides`, a Java
//! artifact's `build_env`) also get one part per key, named `<map>[<key>]`,
//! so the keys that changed can be named. The parts only explain a decision:
//! reuse is still decided by the fingerprint.

use crate::reuse::{
    artifact_docker_build_signature, image_backend_signature, operation_content_signature,
    path_state_signature, resolve_workspace_path, source_backend_signature,
};
use crate::reuse_assembly::assembly_input_signature;
use crate::reuse_imports::import_source_signature;
use crate::reuse_toolchain::artifact_backend_signature;
use crate::{ExecutionPlan, OperationKind, PlannedOperation};
use gaia_spec::{
    ArtifactDefinition, ArtifactSpec, ImageDefinition, ResolvedBuildSpec, SourceDefinition,
    SourceSpec,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// The named inputs of `operation`, as `(name, digest)` pairs in a stable
/// order. Digests are SHA-256 hex of each input's canonical text, so they
/// are safe to store on one line. Dependencies are listed by their content
/// signature; an operation kind without named inputs yields an empty list.
pub fn operation_components(
    spec: &ResolvedBuildSpec,
    plan: &ExecutionPlan,
    operation: &PlannedOperation,
) -> Vec<(String, String)> {
    let mut parts = Parts::default();
    match &operation.kind {
        OperationKind::ResolveBuild => {}
        OperationKind::EmitReport => parts.raw("reporting", format!("{:?}", spec.reporting)),
        OperationKind::MaterializeSource { source_id } => {
            if let Some(source) = spec.sources.iter().find(|source| source.id == *source_id) {
                source_parts(spec, source, &mut parts);
            }
        }
        OperationKind::BuildArtifact { artifact_id } => {
            if let Some(artifact) = spec
                .artifacts
                .iter()
                .find(|artifact| artifact.id == *artifact_id)
            {
                artifact_parts(spec, artifact, &mut parts);
            }
        }
        OperationKind::InstallArtifact { install_id, .. } => {
            if let Some(install) = spec
                .install
                .entries
                .iter()
                .find(|install| install.id == *install_id)
            {
                parts.raw("install entry", format!("{install:?}"));
            }
        }
        OperationKind::RenderStageFile { item_id } => {
            if let Some(item) = spec.stage.files.iter().find(|item| item.id == *item_id) {
                parts.raw("stage entry", format!("{item:?}"));
                parts.raw(
                    "source file",
                    path_state_signature(&resolve_workspace_path(spec, &item.src)),
                );
            }
        }
        OperationKind::RenderStageEnvSet { item_id } => {
            if let Some(item) = spec.stage.env_sets.iter().find(|item| item.id == *item_id) {
                parts.raw("stage entry", format!("{item:?}"));
            }
        }
        OperationKind::RenderStageService { item_id } => {
            if let Some(item) = spec.stage.services.iter().find(|item| item.id == *item_id) {
                parts.raw("stage entry", format!("{item:?}"));
                parts.raw(
                    "unit file",
                    path_state_signature(&resolve_workspace_path(spec, &item.unit_path)),
                );
            }
        }
        OperationKind::PrepareImage | OperationKind::BuildImage => image_parts(spec, &mut parts),
        OperationKind::AssembleImage => {
            parts.raw("assembly", format!("{:?}", spec.image.assembly));
            parts.raw("assembly inputs", assembly_input_signature(spec));
        }
        OperationKind::CaptureCheckpoint { checkpoint_id } => {
            if let Some(checkpoint) = spec
                .checkpoints
                .points
                .iter()
                .find(|checkpoint| checkpoint.id == *checkpoint_id)
            {
                parts.raw("checkpoint", format!("{checkpoint:?}"));
            }
        }
    }

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
        parts.raw(
            format!("{label} {dependency}"),
            content.unwrap_or_else(|| "none".to_string()),
        );
    }
    if let Some(imports) = import_source_signature(spec, &operation.kind) {
        parts.raw("import sources", imports);
    }
    parts.finish()
}

fn source_parts(spec: &ResolvedBuildSpec, source: &SourceSpec, parts: &mut Parts) {
    match &source.definition {
        SourceDefinition::Git(git) => {
            // The checkout identity: the selector, the locked commit and the
            // local repository's HEAD (the backend signature includes git's
            // version, which is rare enough to share this name).
            parts.raw(
                "rev",
                format!(
                    "rev={:?} branch={:?} tag={:?} locked={:?} checkout={}",
                    git.rev,
                    git.branch,
                    git.tag,
                    git.locked_commit,
                    source_backend_signature(spec, source)
                ),
            );
            parts.raw(
                "repo",
                format!(
                    "{:?} subdir={:?} update={} refresh={:?} pin={:?}",
                    git.repo, git.subdir, git.update, git.refresh_policy, git.pin_policy
                ),
            );
        }
        SourceDefinition::Path(_) | SourceDefinition::Archive(_) => {
            parts.raw("definition", format!("{:?}", source.definition));
            parts.raw("materialized tree", source_backend_signature(spec, source));
        }
        SourceDefinition::Download(_) => {
            parts.raw("definition", format!("{:?}", source.definition));
            parts.raw("tool", source_backend_signature(spec, source));
        }
    }
}

fn artifact_parts(spec: &ResolvedBuildSpec, artifact: &ArtifactSpec, parts: &mut Parts) {
    parts.raw("source", format!("{:?}", artifact.source));
    parts.raw("execution", format!("{:?}", artifact.execution));
    parts.raw(
        "toolchain",
        format!(
            "{}|{}",
            artifact_backend_signature(spec, artifact),
            artifact_docker_build_signature(spec, artifact).unwrap_or_default()
        ),
    );
    match &artifact.definition {
        ArtifactDefinition::Java(java) => {
            parts.raw("build target", &java.build_target);
            parts.raw("build args", format!("{:?}", java.build_args));
            parts.raw("build command", format!("{:?}", java.build_command));
            parts.map("build_env", &java.build_env);
        }
        other => parts.raw("definition", format!("{other:?}")),
    }
    parts.raw(
        "options",
        format!(
            "target={:?} build_mode={:?} output={:?} install={:?} after_image_prepare={} dependencies={:?}",
            artifact.target,
            artifact.build_mode,
            artifact.output,
            artifact.install_identity,
            artifact.after_image_prepare,
            artifact.dependencies
        ),
    );
}

fn image_parts(spec: &ResolvedBuildSpec, parts: &mut Parts) {
    let image = &spec.image;
    parts.raw("feed", format!("{:?}", image.feed));
    parts.raw("output", format!("{:?}", image.output));
    match &image.definition {
        ImageDefinition::Buildroot(buildroot) => {
            parts.raw(
                "defconfig",
                format!("{:?}|{:?}", buildroot.defconfig, buildroot.defconfig_path),
            );
            parts.map("config_overrides", &buildroot.config_overrides);
            parts.raw("fragments", format!("{:?}", buildroot.config_fragments));
            parts.raw("buildroot source", format!("{:?}", buildroot.source));
            parts.raw(
                "external trees",
                format!(
                    "{:?}|{:?}",
                    buildroot.external_tree, buildroot.external_tree_mode
                ),
            );
            parts.raw(
                "buildroot options",
                format!(
                    "allow_fallback={:?} expected_images={:?}",
                    buildroot.allow_fallback, buildroot.expected_images
                ),
            );
        }
        ImageDefinition::StartingPoint(starting_point) => {
            parts.raw("starting point", format!("{starting_point:?}"));
        }
    }
    parts.raw("toolchain", image_backend_signature(spec, image));
    let buildroot_policy = &spec.policy.providers.buildroot;
    if buildroot_policy.shared_output {
        parts.raw(
            "shared output",
            format!("{:?}", buildroot_policy.shared_output_dir),
        );
    }
}

/// Collects `(name, canonical text)` pairs and digests them on `finish`.
#[derive(Default)]
struct Parts(Vec<(String, String)>);

impl Parts {
    fn raw(&mut self, name: impl Into<String>, text: impl Into<String>) {
        self.0.push((name.into(), text.into()));
    }

    /// A map-like input: the whole map under `name`, and each key's values
    /// under `name[key]`, so a changed key can be named.
    fn map(&mut self, name: &str, entries: &[(String, String)]) {
        self.raw(name, format!("{entries:?}"));
        let mut keys: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (key, value) in entries {
            keys.entry(key.as_str()).or_default().push(value.as_str());
        }
        for (key, values) in keys {
            self.raw(format!("{name}[{key}]"), format!("{values:?}"));
        }
    }

    fn finish(self) -> Vec<(String, String)> {
        self.0
            .into_iter()
            .map(|(name, text)| (name, digest(&text)))
            .collect()
    }
}

fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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
    fn identical_components_name_nothing() {
        let same = parts(&[("defconfig", "a"), ("build_env[K]", "1")]);
        assert_eq!(fingerprint_change_detail("image", &same, &same), None);
    }
}
