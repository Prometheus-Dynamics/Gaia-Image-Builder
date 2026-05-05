use gaia_artifact_providers::ArtifactProviderCatalog;
use gaia_config::{ResolveOptions, resolve_config_with_options};
use gaia_image_providers::ImageProviderCatalog;
use gaia_plan::{ExecutionPlan, ReuseState, operation_output_signature, spec_fingerprint};
use gaia_source_providers::SourceProviderCatalog;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn default_config_path() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/default-workspace/configs/default.toml")
        .display()
        .to_string()
}

pub fn unique_dir(prefix: &str) -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let count = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir()
        .join("gaia-tests")
        .join(format!("{prefix}-{nonce}-{count}"))
        .display()
        .to_string()
}

pub fn test_spec() -> gaia_spec::ResolvedBuildSpec {
    let build_dir = unique_dir("gaia-report-build");
    let out_dir = unique_dir("gaia-report-out");
    resolve_config_with_options(
        &default_config_path(),
        &ResolveOptions {
            explicit_overrides: vec![
                ("workspace.build_dir".into(), build_dir),
                ("workspace.out_dir".into(), out_dir),
            ],
            ..ResolveOptions::default()
        },
    )
}

pub fn provider_catalogs() -> (
    SourceProviderCatalog,
    ArtifactProviderCatalog,
    ImageProviderCatalog,
) {
    gaia_default_providers::provider_catalogs()
}

pub fn materialize_reusable_outputs(spec: &gaia_spec::ResolvedBuildSpec) {
    fs::create_dir_all(PathBuf::from(&spec.workspace.build_dir).join("sources/gaia-upstream"))
        .expect("gaia-upstream source dir");
    fs::write(
        PathBuf::from(&spec.workspace.build_dir).join("sources/gaia-upstream/source.txt"),
        "ok",
    )
    .expect("gaia-upstream source marker");
    fs::create_dir_all(PathBuf::from(&spec.workspace.build_dir).join("sources/workspace-root"))
        .expect("workspace-root source dir");
    fs::write(
        PathBuf::from(&spec.workspace.build_dir).join("sources/workspace-root/source.txt"),
        "ok",
    )
    .expect("workspace-root source marker");
    if let Some(parent) = PathBuf::from(&spec.artifacts[0].output.path).parent() {
        fs::create_dir_all(parent).expect("artifact output dir");
    }
    fs::write(&spec.artifacts[0].output.path, "artifact").expect("artifact output");
    let collect_dir = spec
        .image
        .output
        .collect_dir
        .clone()
        .expect("image collect dir");
    fs::create_dir_all(&collect_dir).expect("image collect dir create");
    fs::write(
        PathBuf::from(&collect_dir).join("image-provider.txt"),
        "image",
    )
    .expect("image marker");
    let archive_name = spec
        .image
        .output
        .archive_name
        .clone()
        .expect("image archive name");
    fs::write(PathBuf::from(&collect_dir).join(archive_name), "archive").expect("image archive");
    fs::write(
        PathBuf::from(&collect_dir).join(".gaia-image-state.txt"),
        "provider=buildroot\nemit_report=true\narchive=default-2.0.0.tar\nreused=false\n",
    )
    .expect("image provider state");
    let runtime_dir = PathBuf::from(&spec.workspace.out_dir).join(".gaia/runtime");
    fs::create_dir_all(&runtime_dir).expect("runtime dir");
    fs::write(
        runtime_dir.join("install-install-gaia-app.state"),
        "kind=install\ninstall_id=install-gaia-app\nartifact_id=gaia-app\ndest=/usr/bin/default\nreplace=true\nmode=755\nowner=root\ngroup=root\n",
    )
    .expect("install runtime state");
    fs::write(
        runtime_dir.join("stage-file-motd.state"),
        "kind=stage-file\nitem_id=motd\nsrc=assets/motd\ndest=/etc/motd\norigin=static-asset\n",
    )
    .expect("stage file runtime state");
    fs::write(
        runtime_dir.join("stage-env-runtime-env.state"),
        "kind=stage-env\nitem_id=runtime-env\nname=runtime\nentry_count=2\n",
    )
    .expect("stage env runtime state");
    fs::write(
        runtime_dir.join("stage-service-gaia-service.state"),
        "kind=stage-service\nitem_id=gaia-service\nname=gaia\nunit_path=/etc/systemd/system/gaia.service\n",
    )
    .expect("stage service runtime state");
    fs::write(
        runtime_dir.join("checkpoint-base-image.state"),
        "kind=checkpoint\ncheckpoint_id=base-image\nbackend=local\nanchor=image\nuse_policy=Auto\nupload_policy=Off\n",
    )
    .expect("checkpoint runtime state");
}

pub fn reuse_state_for_ids(
    spec: &gaia_spec::ResolvedBuildSpec,
    baseline_plan: &ExecutionPlan,
    reused_ids: &[&str],
) -> ReuseState {
    ReuseState {
        spec_fingerprint: spec_fingerprint(spec),
        completed_operation_ids: reused_ids
            .iter()
            .copied()
            .map(str::to_string)
            .collect::<BTreeSet<_>>(),
        operation_fingerprints: baseline_plan
            .operations
            .iter()
            .filter(|operation| reused_ids.contains(&operation.id.as_str()))
            .map(|operation| (operation.id.as_str().to_string(), operation.fingerprint))
            .collect(),
        operation_output_signatures: baseline_plan
            .operations
            .iter()
            .filter(|operation| reused_ids.contains(&operation.id.as_str()))
            .filter_map(|operation| {
                operation_output_signature(spec, &operation.kind)
                    .map(|signature| (operation.id.as_str().to_string(), signature))
            })
            .collect(),
    }
}
