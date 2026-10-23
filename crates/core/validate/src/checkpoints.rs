use std::collections::HashSet;

use gaia_spec::{CheckpointAnchorRef, CheckpointPolicy, ResolvedBuildSpec};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;

/// The registry settings of a checkpoint. Only `backend = "oci"` takes them;
/// pulls need a verify key (or `trust = "unsigned"`), pushes need a sign key.
fn validate_oci_backend(
    checkpoint: &gaia_spec::CheckpointPointSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let Some(backend) = checkpoint.backend.as_ref() else {
        return;
    };
    let id = checkpoint.id.as_str();
    let subject = Some(format!("checkpoint:{id}"));
    let Some(oci) = backend.oci.as_ref() else {
        return;
    };
    if backend.backend != "oci" {
        diagnostics.push(error(
            "checkpoint_oci_field_without_backend",
            format!(
                "checkpoint '{id}' sets registry fields but its backend is '{}', not 'oci'",
                backend.backend
            ),
            subject,
        ));
        return;
    }
    if oci.registry.trim().is_empty() {
        diagnostics.push(error(
            "checkpoint_oci_registry_missing",
            format!("checkpoint '{id}' (backend oci) needs a registry, such as ghcr.io"),
            subject.clone(),
        ));
    }
    if oci.repository.trim().is_empty() {
        diagnostics.push(error(
            "checkpoint_oci_repository_missing",
            format!(
                "checkpoint '{id}' (backend oci) needs a repository, such as <owner>/gaia-cache"
            ),
            subject.clone(),
        ));
    }
    if let Some(kind) = oci.sign_kind.as_deref()
        && !matches!(kind, "minisign" | "gpg" | "cosign-blob")
    {
        diagnostics.push(error(
            "checkpoint_oci_sign_kind",
            format!("checkpoint '{id}' sign_kind '{kind}' is not minisign, gpg or cosign-blob"),
            subject.clone(),
        ));
    }
    if let Some(trust) = oci.trust.as_deref()
        && trust != "unsigned"
    {
        diagnostics.push(error(
            "checkpoint_oci_trust",
            format!("checkpoint '{id}' trust '{trust}' is not 'unsigned' (the only value)"),
            subject.clone(),
        ));
    }
    let pulls = checkpoint.use_policy != CheckpointPolicy::Off;
    let pushes = checkpoint.upload_policy != CheckpointPolicy::Off;
    let verify_missing = oci
        .verify_key
        .as_deref()
        .is_none_or(|key| key.trim().is_empty());
    if pulls && verify_missing && !oci.trust_unsigned() {
        diagnostics.push(error(
            "checkpoint_oci_verify_key_missing",
            format!(
                "checkpoint '{id}' pulls from the registry but has no verify_key: pulls are refused. Set verify_key, or trust = \"unsigned\" (unsafe)"
            ),
            subject.clone(),
        ));
    }
    if pushes
        && oci
            .sign_key
            .as_deref()
            .is_none_or(|key| key.trim().is_empty())
    {
        diagnostics.push(crate::diagnostics::warning(
            "checkpoint_oci_sign_key_missing",
            format!(
                "checkpoint '{id}' uploads to the registry but has no sign_key: nothing is pushed"
            ),
            subject,
        ));
    }
}

pub(crate) fn validate_checkpoints(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let mut checkpoint_ids = HashSet::new();
    let oci_count = spec
        .checkpoints
        .points
        .iter()
        .filter(|checkpoint| {
            checkpoint
                .backend
                .as_ref()
                .is_some_and(|backend| backend.backend == "oci")
        })
        .count();
    if oci_count > 1 {
        diagnostics.push(error(
            "checkpoint_oci_multiple",
            "only one checkpoint may use backend oci: the registry cache is one repository".into(),
            Some("checkpoint".into()),
        ));
    }
    for checkpoint in &spec.checkpoints.points {
        if !checkpoint.id.is_valid() {
            diagnostics.push(error(
                "checkpoint_id_empty",
                "checkpoint id cannot be empty".into(),
                Some("checkpoint".into()),
            ));
        }
        if !checkpoint_ids.insert(checkpoint.id.as_str().to_string()) {
            diagnostics.push(error(
                "duplicate_checkpoint_id",
                format!("duplicate checkpoint id '{}'", checkpoint.id.as_str()),
                Some(format!("checkpoint:{}", checkpoint.id.as_str())),
            ));
        }
        if checkpoint.backend.is_none()
            && (checkpoint.use_policy != CheckpointPolicy::Off
                || checkpoint.upload_policy != CheckpointPolicy::Off)
        {
            diagnostics.push(error(
                "checkpoint_backend_missing",
                format!(
                    "checkpoint '{}' enables checkpoint policies without selecting a backend",
                    checkpoint.id.as_str()
                ),
                Some(format!("checkpoint:{}", checkpoint.id.as_str())),
            ));
        }
        validate_oci_backend(checkpoint, diagnostics);
        match &checkpoint.anchor {
            CheckpointAnchorRef::Image => {}
            CheckpointAnchorRef::Unknown(raw) => diagnostics.push(error(
                "unknown_checkpoint_anchor",
                format!(
                    "checkpoint '{}' references unknown anchor '{}'",
                    checkpoint.id.as_str(),
                    raw
                ),
                Some(format!("checkpoint:{}", checkpoint.id.as_str())),
            )),
            CheckpointAnchorRef::Install(id) => {
                if !spec.install.entries.iter().any(|install| install.id == *id) {
                    diagnostics.push(error(
                        "unknown_checkpoint_anchor",
                        format!(
                            "checkpoint '{}' references unknown install anchor '{}'",
                            checkpoint.id.as_str(),
                            id.as_str()
                        ),
                        Some(format!("checkpoint:{}", checkpoint.id.as_str())),
                    ));
                }
            }
            CheckpointAnchorRef::StageFile(id) => {
                if !spec.stage.files.iter().any(|file| file.id == *id) {
                    diagnostics.push(error(
                        "unknown_checkpoint_anchor",
                        format!(
                            "checkpoint '{}' references unknown stage-file anchor '{}'",
                            checkpoint.id.as_str(),
                            id.as_str()
                        ),
                        Some(format!("checkpoint:{}", checkpoint.id.as_str())),
                    ));
                }
            }
            CheckpointAnchorRef::StageEnvSet(id) => {
                if !spec.stage.env_sets.iter().any(|env_set| env_set.id == *id) {
                    diagnostics.push(error(
                        "unknown_checkpoint_anchor",
                        format!(
                            "checkpoint '{}' references unknown stage-env anchor '{}'",
                            checkpoint.id.as_str(),
                            id.as_str()
                        ),
                        Some(format!("checkpoint:{}", checkpoint.id.as_str())),
                    ));
                }
            }
            CheckpointAnchorRef::StageService(id) => {
                if !spec.stage.services.iter().any(|service| service.id == *id) {
                    diagnostics.push(error(
                        "unknown_checkpoint_anchor",
                        format!(
                            "checkpoint '{}' references unknown stage-service anchor '{}'",
                            checkpoint.id.as_str(),
                            id.as_str()
                        ),
                        Some(format!("checkpoint:{}", checkpoint.id.as_str())),
                    ));
                }
            }
        }

        if !anchor_is_supported_by_image_flow(spec, &checkpoint.anchor) {
            diagnostics.push(error(
            "illegal_checkpoint_anchor_domain",
            format!(
                "checkpoint '{}' anchors to '{}' but image provider '{:?}' does not include that domain in the active image feed",
                checkpoint.id.as_str(),
                checkpoint.anchor.as_str(),
                spec.image.provider_kind()
            ),
            Some(format!("checkpoint:{}", checkpoint.id.as_str())),
        ));
        }

        if checkpoint_requires_report_ordering(checkpoint)
            && !anchor_is_in_image_dependency_chain(spec, &checkpoint.anchor)
        {
            diagnostics.push(error(
            "checkpoint_anchor_impossible_ordering",
            format!(
                "checkpoint '{}' is '{}' but anchors to '{}' outside the image dependency chain, which would force report ordering on a disconnected branch",
                checkpoint.id.as_str(),
                checkpoint_optionality_label(checkpoint),
                checkpoint.anchor.as_str()
            ),
            Some(format!("checkpoint:{}", checkpoint.id.as_str())),
        ));
        }
    }
}

fn anchor_is_supported_by_image_flow(
    spec: &ResolvedBuildSpec,
    anchor: &CheckpointAnchorRef,
) -> bool {
    match anchor {
        CheckpointAnchorRef::Image => true,
        CheckpointAnchorRef::Install(id) => spec
            .image
            .feed
            .install_entries
            .iter()
            .any(|entry| entry == id),
        CheckpointAnchorRef::StageFile(id) => {
            spec.image.feed.stage_files.iter().any(|entry| entry == id)
        }
        CheckpointAnchorRef::StageEnvSet(id) => spec
            .image
            .feed
            .stage_env_sets
            .iter()
            .any(|entry| entry == id),
        CheckpointAnchorRef::StageService(id) => spec
            .image
            .feed
            .stage_services
            .iter()
            .any(|entry| entry == id),
        CheckpointAnchorRef::Unknown(_) => false,
    }
}

fn anchor_is_in_image_dependency_chain(
    spec: &ResolvedBuildSpec,
    anchor: &CheckpointAnchorRef,
) -> bool {
    anchor_is_supported_by_image_flow(spec, anchor)
}

fn checkpoint_requires_report_ordering(checkpoint: &gaia_spec::CheckpointPointSpec) -> bool {
    !matches!(
        (checkpoint.use_policy, checkpoint.upload_policy),
        (
            gaia_spec::CheckpointPolicy::Off,
            gaia_spec::CheckpointPolicy::Off
        )
    )
}

fn checkpoint_optionality_label(checkpoint: &gaia_spec::CheckpointPointSpec) -> &'static str {
    match (checkpoint.use_policy, checkpoint.upload_policy) {
        (gaia_spec::CheckpointPolicy::Always, _) | (_, gaia_spec::CheckpointPolicy::Always) => {
            "required"
        }
        (gaia_spec::CheckpointPolicy::Auto, _) | (_, gaia_spec::CheckpointPolicy::Auto) => {
            "conditional"
        }
        (gaia_spec::CheckpointPolicy::Off, gaia_spec::CheckpointPolicy::Off) => "best-effort",
    }
}
