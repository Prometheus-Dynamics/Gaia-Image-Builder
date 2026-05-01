use std::collections::HashSet;

use gaia_spec::{CheckpointAnchorRef, CheckpointPolicy, ResolvedBuildSpec};

use crate::ValidationDiagnostic;
use crate::diagnostics::error;

pub(crate) fn validate_checkpoints(
    spec: &ResolvedBuildSpec,
    diagnostics: &mut Vec<ValidationDiagnostic>,
) {
    let mut checkpoint_ids = HashSet::new();
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
