use super::*;

pub(crate) fn compile_checkpoint(raw: RawCheckpointConfig) -> CheckpointPointSpec {
    let backend = compile_checkpoint_backend(&raw);
    CheckpointPointSpec {
        id: CheckpointId::new(raw.id),
        backend,
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

/// The backend of a checkpoint. The registry fields are kept for any backend
/// so validation can reject them on one that is not `oci`.
fn compile_checkpoint_backend(raw: &RawCheckpointConfig) -> Option<CheckpointBackendRef> {
    let backend = raw.backend.clone()?;
    let has_oci_fields = raw.registry.is_some()
        || raw.repository.is_some()
        || raw.verify_key.is_some()
        || raw.sign_key.is_some()
        || raw.sign_kind.is_some()
        || raw.trust.is_some();
    let oci = (backend == "oci" || has_oci_fields).then(|| OciCheckpointSpec {
        registry: raw.registry.clone().unwrap_or_default(),
        repository: raw.repository.clone().unwrap_or_default(),
        verify_key: raw.verify_key.clone(),
        sign_key: raw.sign_key.clone(),
        sign_kind: raw.sign_kind.clone(),
        trust: raw.trust.clone(),
    });
    Some(CheckpointBackendRef { backend, oci })
}
