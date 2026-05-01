use super::*;

pub(crate) fn compile_checkpoint(raw: RawCheckpointConfig) -> CheckpointPointSpec {
    CheckpointPointSpec {
        id: CheckpointId::new(raw.id),
        backend: raw.backend.map(|backend| CheckpointBackendRef { backend }),
        use_policy: compile_checkpoint_policy(raw.use_policy),
        upload_policy: compile_checkpoint_policy(raw.upload_policy),
        anchor: compile_checkpoint_anchor(raw.anchor),
    }
}

pub(crate) fn compile_checkpoint_anchor(raw: Option<String>) -> CheckpointAnchorRef {
    let Some(raw) = raw else {
        return CheckpointAnchorRef::Image;
    };
    if raw == "image" {
        return CheckpointAnchorRef::Image;
    }
    if let Some(id) = raw.strip_prefix("install:") {
        return CheckpointAnchorRef::Install(InstallId::new(id));
    }
    if let Some(id) = raw.strip_prefix("stage-file:") {
        return CheckpointAnchorRef::StageFile(StageItemId::new(id));
    }
    if let Some(id) = raw.strip_prefix("stage-env:") {
        return CheckpointAnchorRef::StageEnvSet(StageItemId::new(id));
    }
    if let Some(id) = raw.strip_prefix("stage-service:") {
        return CheckpointAnchorRef::StageService(StageItemId::new(id));
    }
    CheckpointAnchorRef::Unknown(raw)
}

pub(crate) fn compile_stage_content_origin(
    raw: Option<RawStageContentOrigin>,
) -> StageContentOriginSpec {
    match raw.unwrap_or(RawStageContentOrigin::StaticAsset) {
        RawStageContentOrigin::StaticAsset => StageContentOriginSpec::StaticAsset,
        RawStageContentOrigin::Generated => StageContentOriginSpec::Generated,
        RawStageContentOrigin::ProviderEmitted => StageContentOriginSpec::ProviderEmitted,
    }
}

pub(crate) fn compile_checkpoint_policy(policy: RawCheckpointPolicy) -> CheckpointPolicy {
    match policy {
        RawCheckpointPolicy::Off => CheckpointPolicy::Off,
        RawCheckpointPolicy::Auto => CheckpointPolicy::Auto,
        RawCheckpointPolicy::Always => CheckpointPolicy::Always,
    }
}
