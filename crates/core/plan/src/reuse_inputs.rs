//! The named inputs of an operation: the one list that both the operation
//! fingerprint ([`operation_fingerprint`]) and the recorded components that
//! explain a changed fingerprint (`reuse_details`) are built from, so the two
//! cannot drift apart.
//!
//! Each input is a `(name, canonical text)` pair. Spec structs are split into
//! named inputs from their serialized form: the fields an explanation names
//! (`rev`, `config_overrides`, ...) are taken out into their own input, and
//! what is left of the struct is one more input (`source`, `artifact`,
//! `image`). Every serialized field therefore reaches the fingerprint, also
//! fields added later, without listing them here. Fields a spec leaves out of
//! its serialized form (unset options, where git runs) are not inputs.

use crate::OperationKind;
use crate::reuse::content::config_path_fingerprint;
use crate::reuse::{
    artifact_docker_build_signature, image_backend_signature, operation_output_signature,
    plan_memo, source_backend_signature,
};
use crate::reuse_assembly::assembly_input_entries;
use crate::reuse_imports::import_source_signature;
use crate::reuse_toolchain::artifact_backend_signature;
use gaia_spec::fingerprint::{Fingerprint, canonical_json, canonical_json_value, json_value};
use gaia_spec::{
    ArtifactDefinition, ArtifactSpec, ImageDefinition, ResolvedBuildSpec, SourceDefinition,
    SourceSpec,
};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// The fingerprint of an operation: SHA-256 over its named inputs (see the
/// module docs), truncated to the 64 bits the reuse state records.
pub fn operation_fingerprint(spec: &ResolvedBuildSpec, kind: &OperationKind) -> u64 {
    let mut fingerprint = Fingerprint::new("operation");
    for (name, text) in operation_inputs(spec, kind) {
        fingerprint.field(&name, &text);
    }
    fingerprint.finish().as_u64()
}

/// The named inputs of an operation of `kind`, in a stable order.
pub(crate) fn operation_inputs(
    spec: &ResolvedBuildSpec,
    kind: &OperationKind,
) -> Vec<(String, String)> {
    let mut inputs = Inputs::default();
    match kind {
        OperationKind::ResolveBuild => inputs.json(
            "identity",
            serde_json::json!({
                "build_name": spec.identity.build_name,
                "display_name": spec.identity.display_name,
                "version": spec.identity.version,
            }),
        ),
        OperationKind::EmitReport => inputs.value("reporting", &spec.reporting),
        OperationKind::MaterializeSource { source_id } => {
            if let Some(source) = spec.sources.iter().find(|source| source.id == *source_id) {
                source_inputs(spec, source, &mut inputs);
            }
        }
        OperationKind::BuildArtifact { artifact_id } => {
            if let Some(artifact) = spec
                .artifacts
                .iter()
                .find(|artifact| artifact.id == *artifact_id)
            {
                artifact_inputs(spec, artifact, &mut inputs);
            }
        }
        OperationKind::InstallArtifact { install_id, .. } => {
            if let Some(install) = spec
                .install
                .entries
                .iter()
                .find(|install| install.id == *install_id)
            {
                inputs.value("install entry", install);
            }
        }
        OperationKind::RenderStageFile { item_id } => {
            if let Some(item) = spec.stage.files.iter().find(|item| item.id == *item_id) {
                inputs.value("stage entry", item);
                inputs.text("source file", config_path_fingerprint(spec, &item.src));
            }
        }
        OperationKind::RenderStageEnvSet { item_id } => {
            if let Some(item) = spec.stage.env_sets.iter().find(|item| item.id == *item_id) {
                inputs.value("stage entry", item);
            }
        }
        OperationKind::RenderStageService { item_id } => {
            if let Some(item) = spec.stage.services.iter().find(|item| item.id == *item_id) {
                inputs.value("stage entry", item);
                inputs.text("unit file", config_path_fingerprint(spec, &item.unit_path));
            }
        }
        OperationKind::PrepareImage | OperationKind::BuildImage => {
            image_inputs(spec, matches!(kind, OperationKind::BuildImage), &mut inputs)
        }
        OperationKind::AssembleImage => {
            inputs.value("assembly", &spec.image.assembly);
            // The disk the archive is built from is an input of this step.
            inputs.value("disk", &spec.image.output.disk);
            // Hashes the collected image, which the plan's image:build
            // operation also reads: computed once per plan.
            inputs.text(
                "image build output",
                plan_memo("build-image-output".into(), || {
                    canonical_json(&operation_output_signature(
                        spec,
                        &OperationKind::BuildImage,
                    ))
                }),
            );
            // One input per external input (`assembly inputs[src <path>]`),
            // so the one that changed is named.
            inputs.map("assembly inputs", &assembly_input_entries(spec));
        }
        OperationKind::CheckImage => {
            inputs.value("checks", &spec.image.checks);
            inputs.text(
                "report",
                gaia_spec::image_checks_report_path(spec)
                    .display()
                    .to_string(),
            );
        }
        OperationKind::CaptureCheckpoint { checkpoint_id } => {
            if let Some(checkpoint) = spec
                .checkpoints
                .points
                .iter()
                .find(|checkpoint| checkpoint.id == *checkpoint_id)
            {
                inputs.value("checkpoint", checkpoint);
            }
        }
    }
    if let Some(imports) = import_source_signature(spec, kind) {
        inputs.text("import sources", imports);
    }
    inputs.0
}

fn source_inputs(spec: &ResolvedBuildSpec, source: &SourceSpec, inputs: &mut Inputs) {
    let mut rest = json_value(source);
    match &source.definition {
        SourceDefinition::Git(_) => {
            // The checkout identity: the selector, the locked commit and what
            // the backend resolves (the local repository's HEAD, a floating
            // ref's commit; it includes git's version, which changes rarely
            // enough to share this name).
            let mut rev = take(
                &mut rest,
                "/definition/Git",
                &["rev", "branch", "tag", "locked_commit"],
            );
            if let Value::Object(fields) = &mut rev {
                fields.insert(
                    "checkout".into(),
                    Value::String(source_backend_signature(spec, source)),
                );
            }
            inputs.json("rev", rev);
            inputs.json(
                "repo",
                take(
                    &mut rest,
                    "/definition/Git",
                    &["repo", "subdir", "update", "refresh_policy", "pin_policy"],
                ),
            );
        }
        SourceDefinition::Path(_) | SourceDefinition::Archive(_) => {
            inputs.json("definition", take_field(&mut rest, "", "definition"));
            inputs.text("materialized tree", source_backend_signature(spec, source));
        }
        SourceDefinition::Download(_) => {
            inputs.json("definition", take_field(&mut rest, "", "definition"));
            inputs.text("tool", source_backend_signature(spec, source));
        }
    }
    inputs.json("source", rest);
}

fn artifact_inputs(spec: &ResolvedBuildSpec, artifact: &ArtifactSpec, inputs: &mut Inputs) {
    let mut rest = json_value(artifact);
    inputs.json("source", take_field(&mut rest, "", "source"));
    // Absent (not serialized) when the artifact names no extra sources.
    let extra_sources = take_field(&mut rest, "", "extra_sources");
    if !extra_sources.is_null() {
        inputs.json("extra sources", extra_sources);
    }
    inputs.json("execution", take_field(&mut rest, "", "execution"));
    inputs.text(
        "toolchain",
        format!(
            "{}|{}",
            artifact_backend_signature(spec, artifact),
            artifact_docker_build_signature(spec, artifact).unwrap_or_default()
        ),
    );
    match &artifact.definition {
        ArtifactDefinition::Java(java) => {
            let definition = "/definition/Java";
            inputs.json(
                "build target",
                take_field(&mut rest, definition, "build_target"),
            );
            inputs.json(
                "build args",
                take_field(&mut rest, definition, "build_args"),
            );
            inputs.json(
                "build command",
                take_field(&mut rest, definition, "build_command"),
            );
            // The contents of build command scripts (see reuse_build_inputs).
            for (name, digest) in
                crate::reuse_build_inputs::build_command_components(spec, artifact)
            {
                inputs.text(name, digest);
            }
            take_field(&mut rest, definition, "build_env");
            inputs.map("build_env", &java.build_env);
        }
        _ => inputs.json("definition", take_field(&mut rest, "", "definition")),
    }
    inputs.json(
        "options",
        take(
            &mut rest,
            "",
            &[
                "target",
                "build_mode",
                "output",
                "install_identity",
                "after_image_prepare",
                "dependencies",
            ],
        ),
    );
    inputs.json("artifact", rest);
}

fn image_inputs(spec: &ResolvedBuildSpec, include_post_image: bool, inputs: &mut Inputs) {
    let image = &spec.image;
    let mut rest = json_value(image);
    // Disk assembly is fingerprinted by its own operation; a partition
    // layout change must not re-run the Buildroot build.
    take_field(&mut rest, "", "assembly");
    // The image checks are fingerprinted by their own operation.
    take_field(&mut rest, "", "checks");
    let buildroot_definition = "/definition/Buildroot";
    // Where and under what name images are collected and archived, the image
    // feed (installs, stage files, services) and leaving packages out of the
    // rootfs are all applied by image:build only: the prepare step neither
    // collects nor archives, runs no feed and finalizes without exclusions.
    let feed = take_field(&mut rest, "", "feed");
    let output = take_field(&mut rest, "", "output");
    let rootfs_exclude = take_field(&mut rest, buildroot_definition, "rootfs_exclude_packages");
    if include_post_image {
        inputs.json("feed", feed);
        inputs.json("output", output);
        if rootfs_exclude
            .as_array()
            .is_some_and(|packages| !packages.is_empty())
        {
            inputs.json("rootfs exclude packages", rootfs_exclude);
        }
    }
    match &image.definition {
        ImageDefinition::Buildroot(buildroot) => {
            inputs.json(
                "defconfig",
                take(
                    &mut rest,
                    buildroot_definition,
                    &["defconfig", "defconfig_path"],
                ),
            );
            take_field(&mut rest, buildroot_definition, "config_overrides");
            inputs.map("config_overrides", &buildroot.config_overrides);
            inputs.json(
                "fragments",
                take_field(&mut rest, buildroot_definition, "config_fragments"),
            );
            inputs.json(
                "buildroot source",
                take_field(&mut rest, buildroot_definition, "source"),
            );
            inputs.json(
                "external trees",
                take(
                    &mut rest,
                    buildroot_definition,
                    &["external_tree", "external_tree_mode"],
                ),
            );
            inputs.json(
                "buildroot options",
                take(
                    &mut rest,
                    buildroot_definition,
                    &["allow_fallback", "expected_images"],
                ),
            );
        }
        ImageDefinition::StartingPoint(_) => {
            inputs.json("starting point", take_field(&mut rest, "", "definition"));
        }
    }
    inputs.json("image", rest);
    inputs.text("toolchain", image_backend_signature(spec, image));
    // Post scripts and BR2_EXTERNAL files by content (see reuse_build_inputs).
    for (name, digest) in
        crate::reuse_build_inputs::buildroot_image_components(spec, include_post_image)
    {
        inputs.text(name, digest);
    }
    let buildroot_policy = &spec.policy.providers.buildroot;
    if buildroot_policy.shared_output {
        inputs.value("shared output", &buildroot_policy.shared_output_dir);
    }
}

/// Removes `key` from the object at JSON pointer `parent` (`""` for the
/// root) and returns its value, `Null` when absent.
fn take_field(value: &mut Value, parent: &str, key: &str) -> Value {
    value
        .pointer_mut(parent)
        .and_then(Value::as_object_mut)
        .and_then(|object| object.remove(key))
        .unwrap_or(Value::Null)
}

/// Removes `keys` from the object at `parent` and returns the present ones
/// as one object.
fn take(value: &mut Value, parent: &str, keys: &[&str]) -> Value {
    let mut taken = Map::new();
    for key in keys {
        let field = take_field(value, parent, key);
        if !field.is_null() {
            taken.insert((*key).to_string(), field);
        }
    }
    Value::Object(taken)
}

/// Collects `(name, canonical text)` pairs.
#[derive(Default)]
struct Inputs(Vec<(String, String)>);

impl Inputs {
    fn text(&mut self, name: impl Into<String>, text: impl Into<String>) {
        self.0.push((name.into(), text.into()));
    }

    fn json(&mut self, name: &str, value: Value) {
        self.text(name, canonical_json_value(&value));
    }

    fn value<T: serde::Serialize + ?Sized>(&mut self, name: &str, value: &T) {
        self.text(name, canonical_json(value));
    }

    /// A map-like input: the whole map under `name`, and each key's values
    /// under `name[key]`, so a changed key can be named.
    fn map(&mut self, name: &str, entries: &[(String, String)]) {
        self.value(name, entries);
        let mut keys: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (key, value) in entries {
            keys.entry(key.as_str()).or_default().push(value.as_str());
        }
        for (key, values) in keys {
            self.value(&format!("{name}[{key}]"), &values);
        }
    }
}

#[cfg(test)]
#[path = "reuse_inputs_tests.rs"]
mod tests;
