use gaia_artifact_providers::ArtifactProviderCatalog;
use gaia_config::{ResolveOptions, resolve_config_with_options};
use gaia_image_providers::ImageProviderCatalog;
use gaia_plan::{ExecutionPlan, ReuseState, operation_output_signature, spec_fingerprint};
use gaia_source_providers::{SourceProvider, SourceProviderCatalog};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    let build_dir = unique_dir("gaia-exec-build");
    let out_dir = unique_dir("gaia-exec-out");
    resolve_config_with_options(
        &default_config_path(),
        &ResolveOptions {
            explicit_overrides: vec![
                ("workspace.build_dir".into(), build_dir),
                ("workspace.out_dir".into(), out_dir),
                ("image.allow_fallback".into(), "true".into()),
                (
                    "policy.providers.rust.allow_nested_build".into(),
                    "true".into(),
                ),
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

pub fn failing_spec() -> gaia_spec::ResolvedBuildSpec {
    failing_spec_with_overrides(Vec::new())
}

pub fn artifact_failure_spec_with_overrides(
    mut extra_overrides: Vec<(String, String)>,
) -> gaia_spec::ResolvedBuildSpec {
    let build_dir = unique_dir("gaia-exec-artifact-fail-build");
    let out_dir = unique_dir("gaia-exec-artifact-fail-out");
    let root_dir = unique_dir("gaia-exec-artifact-fail-root");
    fs::create_dir_all(PathBuf::from(&root_dir).join("src")).expect("artifact fail root src");
    fs::write(
        PathBuf::from(&root_dir).join("Cargo.toml"),
        "[package]\nname = \"gaia\"\nversion = \"2.0.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"gaia\"\npath = \"src/main.rs\"\n",
    )
    .expect("artifact fail cargo toml");
    fs::write(
        PathBuf::from(&root_dir).join("src/main.rs"),
        "fn main() { println!(\"ok\"); }\n",
    )
    .expect("artifact fail main");

    let mut explicit_overrides = vec![
        ("workspace.root_dir".into(), root_dir),
        ("workspace.build_dir".into(), build_dir),
        ("workspace.out_dir".into(), out_dir),
        (
            "policy.providers.rust.allow_nested_build".into(),
            "true".into(),
        ),
    ];
    explicit_overrides.append(&mut extra_overrides);

    let mut spec = resolve_config_with_options(
        &default_config_path(),
        &ResolveOptions {
            explicit_overrides,
            ..ResolveOptions::default()
        },
    );
    let mut bad_artifact = spec.artifacts[0].clone();
    bad_artifact.id = gaia_spec::ArtifactId::new("gaia-bad");
    bad_artifact.output.path = PathBuf::from(&spec.workspace.out_dir)
        .join("artifacts/gaia-bad")
        .display()
        .to_string();
    bad_artifact.definition = gaia_spec::ArtifactDefinition::Rust(gaia_spec::RustArtifactSpec {
        package: "missing-package".into(),
        target_name: Some("gaia-bad".into()),
        variant: gaia_spec::ArtifactVariantSpec::File,
    });
    spec.artifacts.push(bad_artifact);
    spec
}

pub fn failing_spec_with_overrides(
    mut extra_overrides: Vec<(String, String)>,
) -> gaia_spec::ResolvedBuildSpec {
    let build_dir = unique_dir("gaia-exec-fail-build");
    let out_dir = unique_dir("gaia-exec-fail-out");
    let missing_root = unique_dir("gaia-exec-missing-root");
    let mut explicit_overrides = vec![
        ("workspace.root_dir".into(), missing_root),
        ("workspace.build_dir".into(), build_dir),
        ("workspace.out_dir".into(), out_dir),
    ];
    explicit_overrides.append(&mut extra_overrides);
    resolve_config_with_options(
        &default_config_path(),
        &ResolveOptions {
            preset: Some("ci".into()),
            explicit_overrides,
            ..ResolveOptions::default()
        },
    )
}

pub fn materialize_reusable_outputs(spec: &gaia_spec::ResolvedBuildSpec) {
    fs::create_dir_all(Path::new(&spec.workspace.build_dir).join("sources/gaia-upstream"))
        .expect("gaia-upstream source dir");
    fs::write(
        Path::new(&spec.workspace.build_dir).join("sources/gaia-upstream/source.txt"),
        "ok",
    )
    .expect("gaia-upstream source marker");
    fs::create_dir_all(Path::new(&spec.workspace.build_dir).join("sources/workspace-root"))
        .expect("workspace-root source dir");
    fs::write(
        Path::new(&spec.workspace.build_dir).join("sources/workspace-root/source.txt"),
        "ok",
    )
    .expect("workspace-root source marker");
    if let Some(parent) = Path::new(&spec.workspace.out_dir)
        .join("artifacts/gaia")
        .parent()
    {
        fs::create_dir_all(parent).expect("artifact output dir");
    }
    fs::write(
        Path::new(&spec.workspace.out_dir).join("artifacts/gaia"),
        "artifact",
    )
    .expect("artifact output");
    let image_dir = Path::new(&spec.workspace.out_dir).join("images");
    fs::create_dir_all(&image_dir).expect("image output dir");
    fs::write(image_dir.join("image-provider.txt"), "image").expect("image marker");
    fs::write(image_dir.join("default-2.0.0.tar"), "archive").expect("image archive");
    let runtime_dir = Path::new(&spec.workspace.out_dir).join(".gaia/runtime");
    fs::create_dir_all(&runtime_dir).expect("runtime dir");
    fs::write(
        runtime_dir.join("install-install-gaia-app.state"),
        "kind=install\ninstall_id=install-gaia-app\nartifact_id=gaia-app\ndest=/usr/bin/default\nreplace=true\nmode=755\nowner=root\ngroup=root\n",
    )
    .expect("install runtime state");
    fs::write(
        runtime_dir.join("stage-file-motd.state"),
        "kind=stage-file\nitem_id=motd\nsrc=assets/motd\ndest=/etc/motd\n",
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

pub struct SleepPathSourceProvider;

impl SourceProvider for SleepPathSourceProvider {
    fn id(&self) -> &'static str {
        "source.path.sleep"
    }

    fn kind(&self) -> gaia_spec::SourceProviderKind {
        gaia_spec::SourceProviderKind::Path
    }

    fn execute_source(
        &self,
        spec: &gaia_spec::ResolvedBuildSpec,
        source: &gaia_spec::SourceSpec,
        log_sink: Option<gaia_source_providers::ProcessLogSink>,
        cancel_check: Option<gaia_source_providers::ProcessCancelCheck>,
    ) -> Result<Vec<String>, gaia_source_providers::SourceProviderError> {
        let _ = log_sink;
        let _ = cancel_check;
        thread::sleep(Duration::from_millis(300));
        let source_dir = Path::new(&spec.workspace.build_dir)
            .join("sources")
            .join(source.id.as_str());
        fs::create_dir_all(&source_dir).expect("parallel source dir");
        fs::write(source_dir.join("source.txt"), "ok").expect("parallel source marker");
        Ok(vec![format!("slept for {}", source.id.as_str())])
    }
}
