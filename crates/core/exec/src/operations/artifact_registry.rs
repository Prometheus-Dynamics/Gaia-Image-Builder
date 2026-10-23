//! The registry cache of artifact outputs. A built artifact is pushed under its
//! operation fingerprint, the key the plan's reuse decides with. An artifact the
//! plan would build is first restored from its entry when that verifies: the
//! output and its provider state are then the bytes a local build leaves, so
//! the local reuse accepts them on the next plan.
//!
//! Pulls and pushes are best-effort: a failure is a message of the run and the
//! artifact is built as it would be without a registry. A signature that does
//! not verify refuses the entry, with a message that says so.
//!
//! An entry is `<id>.json` (a record: artifact id, key, whether the output is a
//! directory, whether a separate state file came with it) and `<id>/output`
//! (the output file or tree) with `<id>/state` for a file output's state.
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use gaia_image_providers::{RegistryCacheUse, registry_cache_from_spec};
use gaia_plan::{OperationKind, operation_fingerprint};
use gaia_process::{PushOutcome, RegistryError, RemoteCache, pack_entry, unpack_entry};
use gaia_spec::{ArtifactSpec, ResolvedBuildSpec};

/// The registry kind of artifact entries (the image is `<repository>/artifact`).
pub(crate) const ARTIFACT_KIND: &str = "artifact";
/// Annotations of an artifact entry: the artifact's id and the stem of its
/// record and archive (both the artifact id).
const ARTIFACT_ANNOTATION: &str = "dev.gaia.artifact";
const STEM_ANNOTATION: &str = "dev.gaia.stem";

/// What the registry did before an artifact build.
pub(crate) struct Restore {
    /// The output and its state were restored: the build is not run.
    pub(crate) restored: bool,
    /// Run messages: the registry settings' warnings, and what the pull did.
    pub(crate) messages: Vec<String>,
}

/// The registry key of `artifact`: a sha256 (64 lower-case hex digits) over
/// its build's operation fingerprint, its sources' content identities (the
/// git commit, the path tree's file contents, wherever they sit) and,
/// recursively, the artifacts it depends on. The build's own fingerprint does
/// not carry its sources' content (the plan rebuilds it because its source
/// re-ran), so a key without them would pull an entry built from other
/// source content; and the sources' plan fingerprints include their location,
/// so they would keep two checkouts of the same content apart.
pub(crate) fn registry_key(spec: &ResolvedBuildSpec, artifact: &ArtifactSpec) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"gaia-artifact-key-v2\n");
    hash_upstream(spec, artifact, &mut hasher, &mut BTreeSet::new());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Feeds `artifact`'s fingerprint, its sources' and (once each) its
/// dependencies' into `hasher`, in a fixed order.
fn hash_upstream(
    spec: &ResolvedBuildSpec,
    artifact: &ArtifactSpec,
    hasher: &mut Sha256,
    seen: &mut BTreeSet<String>,
) {
    if !seen.insert(artifact.id.as_str().to_string()) {
        return;
    }
    let build = OperationKind::BuildArtifact {
        artifact_id: artifact.id.clone(),
    };
    hasher.update(
        format!(
            "artifact {} {:016x}\n",
            artifact.id.as_str(),
            operation_fingerprint(spec, &build)
        )
        .as_bytes(),
    );
    let mut sources: Vec<&gaia_spec::SourceId> = artifact
        .source
        .iter()
        .map(|source| &source.id)
        .chain(&artifact.extra_sources)
        .collect();
    sources.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    sources.dedup();
    for source_id in sources {
        let identity = spec
            .sources
            .iter()
            .find(|source| source.id == *source_id)
            .map(|source| gaia_plan::source_content_identity(spec, source))
            .unwrap_or_else(|| "undefined".into());
        hasher.update(format!("source {} {identity}\n", source_id.as_str()).as_bytes());
    }
    for dependency in &artifact.dependencies {
        if let Some(upstream) = spec
            .artifacts
            .iter()
            .find(|candidate| candidate.id == dependency.id)
        {
            hash_upstream(spec, upstream, hasher, seen);
        }
    }
}

/// Restores `artifact` from the registry when its entry verifies. Called before
/// the artifact is built.
pub(crate) fn restore(spec: &ResolvedBuildSpec, artifact: &ArtifactSpec) -> Restore {
    let (registry, mut messages) = match registry_cache_from_spec(spec) {
        Ok(registry) => {
            let warnings = registry
                .iter()
                .flat_map(|registry| registry.warnings.iter().cloned())
                .collect();
            (registry, warnings)
        }
        Err(error) => (
            None,
            vec![format!(
                "registry cache: {error}; the artifact is built locally"
            )],
        ),
    };
    let result = restore_with(registry.as_ref(), spec, artifact);
    messages.extend(result.messages);
    Restore {
        restored: result.restored,
        messages,
    }
}

/// [`restore`] with the registry cache given.
fn restore_with(
    registry: Option<&RegistryCacheUse>,
    spec: &ResolvedBuildSpec,
    artifact: &ArtifactSpec,
) -> Restore {
    let Some(registry) = registry.filter(|registry| registry.pull) else {
        return Restore {
            restored: false,
            messages: Vec::new(),
        };
    };
    let id = artifact.id.as_str();
    let Some(stem) = entry_stem(artifact) else {
        return Restore {
            restored: false,
            messages: vec![format!(
                "registry pull of artifact '{id}' skipped: its id is not an entry name"
            )],
        };
    };
    let key = registry_key(spec, artifact);
    let output = output_path(spec, artifact);
    match pull(&registry.client(), artifact, &stem, &key, &output) {
        Ok(true) => Restore {
            restored: true,
            messages: vec![format!(
                "artifact '{id}': restored from the registry checkpoint '{}' ({key})",
                registry.checkpoint_id
            )],
        },
        Ok(false) => Restore {
            restored: false,
            messages: Vec::new(),
        },
        Err(reason) => {
            tracing::warn!(artifact = id, key = %key, "registry pull: {reason}");
            Restore {
                restored: false,
                messages: vec![format!(
                    "registry pull of artifact '{id}' ({key}): {reason}"
                )],
            }
        }
    }
}

/// Pushes the output `artifact` has just built, when the spec's registry cache
/// pushes. Returns the run messages (none when nothing was pushed).
pub(crate) fn publish(spec: &ResolvedBuildSpec, artifact: &ArtifactSpec) -> Vec<String> {
    match registry_cache_from_spec(spec) {
        Ok(registry) => publish_with(registry.as_ref(), spec, artifact),
        // The settings' problem was reported before the build.
        Err(_) => Vec::new(),
    }
}

/// [`publish`] with the registry cache given.
fn publish_with(
    registry: Option<&RegistryCacheUse>,
    spec: &ResolvedBuildSpec,
    artifact: &ArtifactSpec,
) -> Vec<String> {
    let Some(registry) = registry.filter(|registry| registry.push) else {
        return Vec::new();
    };
    let id = artifact.id.as_str();
    let Some(stem) = entry_stem(artifact) else {
        return vec![format!(
            "registry push of artifact '{id}' skipped: its id is not an entry name"
        )];
    };
    let key = registry_key(spec, artifact);
    let output = output_path(spec, artifact);
    match push(&registry.client(), artifact, &stem, &key, &output) {
        Ok(PushOutcome::Pushed { replaced, .. }) => {
            if replaced {
                tracing::warn!(
                    artifact = id,
                    key = %key,
                    "registry entry whose signature did not verify was replaced with this build's signed entry"
                );
            }
            vec![format!(
                "artifact '{id}': pushed to the registry checkpoint '{}' ({key}){}",
                registry.checkpoint_id,
                if replaced {
                    ", replacing an entry whose signature did not verify"
                } else {
                    ""
                }
            )]
        }
        Ok(PushOutcome::AlreadyPresent | PushOutcome::Disabled) => Vec::new(),
        Err(reason) => {
            tracing::warn!(
                artifact = id,
                key = %key,
                "registry push failed (the build is not affected): {reason}"
            );
            vec![format!(
                "registry push of artifact '{id}' failed (the build is not affected): {reason}"
            )]
        }
    }
}

/// The artifact's id as an entry name: no path separator, not `..`.
fn entry_stem(artifact: &ArtifactSpec) -> Option<String> {
    let id = artifact.id.as_str();
    (!id.is_empty() && !id.contains('/') && id != "..").then(|| id.to_string())
}

/// The artifact's output on disk, resolved as the plan resolves it.
fn output_path(spec: &ResolvedBuildSpec, artifact: &ArtifactSpec) -> PathBuf {
    let raw = artifact.output.path.as_str();
    gaia_spec::resolve_workspace_path(&spec.workspace, raw).unwrap_or_else(|_| {
        let path = PathBuf::from(raw);
        if path.is_absolute() {
            path
        } else {
            PathBuf::from(&spec.workspace.root_dir).join(path)
        }
    })
}

/// Where a file output's provider state sits: beside it, in `.gaia`. (A
/// directory output keeps its state inside its tree, so it is not here.)
fn state_beside(output: &Path) -> PathBuf {
    let name = output
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("artifact");
    output
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".gaia")
        .join(format!("{name}.gaia-state.txt"))
}

fn push(
    client: &RemoteCache,
    artifact: &ArtifactSpec,
    stem: &str,
    key: &str,
    output: &Path,
) -> Result<PushOutcome, String> {
    if !output.exists() {
        return Err(format!("the output '{}' is not on disk", output.display()));
    }
    let parent = output
        .parent()
        .ok_or_else(|| "the output has no parent directory".to_string())?;
    let staging = parent.join(format!(".gaia-push-{stem}"));
    let _ = fs::remove_dir_all(&staging);
    let result = push_from(client, artifact, stem, key, output, &staging);
    let _ = fs::remove_dir_all(&staging);
    result
}

fn push_from(
    client: &RemoteCache,
    artifact: &ArtifactSpec,
    stem: &str,
    key: &str,
    output: &Path,
    staging: &Path,
) -> Result<PushOutcome, String> {
    let payload = staging.join("payload");
    let entry = payload.join(stem);
    fs::create_dir_all(&entry).map_err(|error| error.to_string())?;
    copy_tree(output, &entry.join("output"))?;
    let is_dir = output.is_dir();
    let state = state_beside(output);
    let has_state = !is_dir && state.is_file();
    if has_state {
        copy_tree(&state, &entry.join("state"))?;
    }
    let record = serde_json::json!({
        "artifact": artifact.id.as_str(),
        "key": key,
        "dir": is_dir,
        "state": has_state,
    });
    let record_name = format!("{stem}.json");
    let text = serde_json::to_string_pretty(&record).map_err(|error| error.to_string())?;
    fs::write(payload.join(&record_name), format!("{text}\n"))
        .map_err(|error| error.to_string())?;
    let blob = staging.join("entry.tar.zst");
    pack_entry(&payload, &[stem, record_name.as_str()], &blob)
        .map_err(|error| error.to_string())?;
    let annotations = BTreeMap::from([
        (
            ARTIFACT_ANNOTATION.to_string(),
            artifact.id.as_str().to_string(),
        ),
        (STEM_ANNOTATION.to_string(), stem.to_string()),
    ]);
    client
        .push(ARTIFACT_KIND, key, &blob, &annotations)
        .map_err(|error| error.to_string())
}

/// Pulls the entry for `key` and restores its output (and state) at `output`
/// when it verifies. `Ok(false)` when the registry has no such entry.
fn pull(
    client: &RemoteCache,
    artifact: &ArtifactSpec,
    stem: &str,
    key: &str,
    output: &Path,
) -> Result<bool, String> {
    let parent = output
        .parent()
        .ok_or_else(|| "the output has no parent directory".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let staging = parent.join(format!(".gaia-pull-{stem}"));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|error| error.to_string())?;
    let result = pull_into(client, artifact, stem, key, output, &staging);
    let _ = fs::remove_dir_all(&staging);
    result
}

fn pull_into(
    client: &RemoteCache,
    artifact: &ArtifactSpec,
    stem: &str,
    key: &str,
    output: &Path,
    staging: &Path,
) -> Result<bool, String> {
    let entry = match client.pull(ARTIFACT_KIND, key, staging) {
        Ok(Some(entry)) => entry,
        Ok(None) => return Ok(false),
        Err(RegistryError::Signature(reason)) => {
            return Err(format!(
                "SIGNATURE DOES NOT VERIFY, entry refused, the artifact is built instead: {reason}"
            ));
        }
        Err(error) => return Err(error.to_string()),
    };
    let annotation = |field: &str| entry.annotations.get(field).map(String::as_str);
    if annotation(ARTIFACT_ANNOTATION) != Some(artifact.id.as_str())
        || annotation(STEM_ANNOTATION) != Some(stem)
    {
        return Err("the entry is for another artifact".into());
    }
    let unpacked = staging.join("unpacked");
    unpack_entry(&entry.blob, &unpacked, stem).map_err(|error| error.to_string())?;
    let text = fs::read_to_string(unpacked.join(format!("{stem}.json")))
        .map_err(|error| error.to_string())?;
    let record: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| "the record is unreadable".to_string())?;
    if record["artifact"] != artifact.id.as_str() || record["key"] != key {
        return Err("the record is for another artifact or key".into());
    }
    let is_dir = record["dir"]
        .as_bool()
        .ok_or_else(|| "the record does not say whether the output is a directory".to_string())?;
    let has_state = record["state"].as_bool().unwrap_or(false);
    let entry_dir = unpacked.join(stem);
    let payload = entry_dir.join("output");
    if !payload.exists() || payload.is_dir() != is_dir {
        return Err("the entry has no output of the recorded kind".into());
    }
    // Everything verified: the output and its state replace what is there.
    remove_existing(output)?;
    fs::rename(&payload, output).map_err(|error| error.to_string())?;
    if !is_dir {
        let state = state_beside(output);
        remove_existing(&state)?;
        if has_state {
            if let Some(dir) = state.parent() {
                fs::create_dir_all(dir).map_err(|error| error.to_string())?;
            }
            fs::rename(entry_dir.join("state"), &state).map_err(|error| error.to_string())?;
        }
    }
    Ok(true)
}

/// Removes `path` (a file, a link, or a directory tree) when it exists.
fn remove_existing(path: &Path) -> Result<(), String> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
    .map_err(|error| format!("failed to replace '{}': {error}", path.display()))
}

/// Copies a file or a directory tree to `to`, which must not exist. Links and
/// special files are refused: an entry carries neither.
fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(from)
        .map_err(|error| format!("failed to stat '{}': {error}", from.display()))?;
    if meta.file_type().is_symlink() {
        return Err(format!(
            "'{}' is a link, which an entry does not carry",
            from.display()
        ));
    }
    if meta.is_dir() {
        fs::create_dir(to).map_err(|error| error.to_string())?;
        let mut children: Vec<_> = fs::read_dir(from)
            .map_err(|error| format!("failed to read '{}': {error}", from.display()))?
            .filter_map(Result::ok)
            .collect();
        children.sort_by_key(|child| child.file_name());
        for child in children {
            copy_tree(&child.path(), &to.join(child.file_name()))?;
        }
        fs::set_permissions(to, meta.permissions()).map_err(|error| error.to_string())
    } else if meta.is_file() {
        fs::copy(from, to)
            .map(drop)
            .map_err(|error| format!("failed to copy '{}': {error}", from.display()))
    } else {
        Err(format!(
            "'{}' is a special file, which an entry does not carry",
            from.display()
        ))
    }
}

#[cfg(test)]
#[path = "artifact_registry_tests.rs"]
mod registry_tests;
