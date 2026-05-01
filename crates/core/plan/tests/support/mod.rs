use gaia_artifact_providers::ArtifactProviderCatalog;
use gaia_config::{ResolveOptions, resolve_config_with_options};
use gaia_image_providers::ImageProviderCatalog;
use gaia_plan::{ExecutionPlan, ReuseState, operation_output_signature, spec_fingerprint};
use gaia_source_providers::SourceProviderCatalog;
use std::collections::BTreeSet;
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

pub fn provider_catalogs() -> (
    SourceProviderCatalog,
    ArtifactProviderCatalog,
    ImageProviderCatalog,
) {
    gaia_default_providers::provider_catalogs()
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
    let build_dir = unique_dir("gaia-plan-build");
    let out_dir = unique_dir("gaia-plan-out");
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

pub fn test_spec_with_root(root_dir: String) -> gaia_spec::ResolvedBuildSpec {
    let build_dir = unique_dir("gaia-plan-build");
    let out_dir = unique_dir("gaia-plan-out");
    resolve_config_with_options(
        &default_config_path(),
        &ResolveOptions {
            explicit_overrides: vec![
                ("workspace.root_dir".into(), root_dir),
                ("workspace.build_dir".into(), build_dir),
                ("workspace.out_dir".into(), out_dir),
            ],
            ..ResolveOptions::default()
        },
    )
}

pub fn reuse_state_for_ids(
    spec: &gaia_spec::ResolvedBuildSpec,
    baseline_plan: &ExecutionPlan,
    reused_ids: &[&str],
) -> ReuseState {
    reuse_state_for_ids_with_fingerprint(spec, baseline_plan, reused_ids, spec_fingerprint(spec))
}

pub fn reuse_state_for_ids_with_fingerprint(
    spec: &gaia_spec::ResolvedBuildSpec,
    baseline_plan: &ExecutionPlan,
    reused_ids: &[&str],
    fingerprint: u64,
) -> ReuseState {
    ReuseState {
        spec_fingerprint: fingerprint,
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
