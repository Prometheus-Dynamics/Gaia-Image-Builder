//! Per-plan signature caching: the cached signatures are the public functions'
//! values, each content hash is made once per plan, and a plan memo lives only
//! as long as the plan it was made for.

use super::*;
use crate::{ExecutionPlan, ReuseState, plan_build_with_reuse_state};
use gaia_config::{ResolveOptions, resolve_config_with_options};
use gaia_spec::{
    InstallEntrySpec, ResolvedBuildSpec, StageContentOriginSpec, StageFileSpec,
    source_build_dir_by_id,
};
use gaia_testkit::TempDir;
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

const INSTALLS: usize = 6;
const STAGE_FILES: usize = 6;

fn default_config_path() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/default-workspace/configs/default.toml")
        .display()
        .to_string()
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    fs::write(path, contents).expect("fixture");
}

/// A default build with `INSTALLS` installs of its artifact and `STAGE_FILES`
/// stage files, every output materialized as a finished run leaves it, and
/// the baseline plan of that build.
fn finished_build(scratch: &TempDir) -> (ResolvedBuildSpec, ExecutionPlan) {
    let mut spec = resolve_config_with_options(
        &default_config_path(),
        &ResolveOptions {
            explicit_overrides: vec![
                (
                    "workspace.build_dir".into(),
                    scratch.join("build").display().to_string(),
                ),
                (
                    "workspace.out_dir".into(),
                    scratch.join("out").display().to_string(),
                ),
            ],
            ..ResolveOptions::default()
        },
    );
    let artifact = spec.artifacts[0].clone();
    write(
        Path::new(&artifact.output.path),
        &"artifact bytes\n".repeat(1 << 16),
    );
    for index in 0..INSTALLS {
        spec.install.entries.push(InstallEntrySpec::new(
            format!("install-sig-{index}"),
            artifact.id.clone(),
            format!("/usr/bin/sig-{index}"),
        ));
    }
    for index in 0..STAGE_FILES {
        let src = scratch.join(format!("assets/sig-{index}.txt"));
        write(&src, &format!("payload {index}\n"));
        spec.stage.files.push(StageFileSpec::new(
            format!("sig-file-{index}"),
            src.display().to_string(),
            format!("/etc/sig-{index}"),
            StageContentOriginSpec::StaticAsset,
        ));
    }
    for source in &spec.sources {
        write(
            &source_build_dir_by_id(&spec, source.id.as_str()).join("source.txt"),
            "ok",
        );
    }
    let collect = spec.image.output.collect_dir.clone().expect("collect dir");
    write(&Path::new(&collect).join("image-provider.txt"), "image");
    let archive = spec.image.output.archive_name.clone().expect("archive");
    write(&Path::new(&collect).join(archive), "archive");
    fs::create_dir_all(
        PathBuf::from(&spec.workspace.build_dir).join("image/buildroot-output/target"),
    )
    .expect("buildroot target");
    let runtime = PathBuf::from(&spec.workspace.out_dir).join(".gaia/runtime");
    for install in &spec.install.entries {
        write(
            &runtime.join(format!("install-{}.state", install.id.as_str())),
            &format!("kind=install\ninstall_id={}\n", install.id.as_str()),
        );
    }
    for item in &spec.stage.files {
        write(
            &runtime.join(format!("stage-file-{}.state", item.id.as_str())),
            &format!("kind=stage-file\nitem_id={}\n", item.id.as_str()),
        );
    }
    let (sources, artifacts, images) = gaia_default_providers::provider_catalogs();
    let baseline = plan_build_with_reuse_state(&spec, &sources, &artifacts, &images, None);
    (spec, baseline)
}

/// What a finished run records for every operation of `baseline`.
fn recorded_state(spec: &ResolvedBuildSpec, baseline: &ExecutionPlan) -> ReuseState {
    let operations = baseline
        .operations
        .iter()
        .filter(|operation| {
            operation.id.as_str() != "resolve-build" && operation.id.as_str() != "report:emit"
        })
        .collect::<Vec<_>>();
    ReuseState {
        spec_fingerprint: spec_fingerprint(spec),
        completed_operation_ids: operations
            .iter()
            .map(|operation| operation.id.as_str().to_string())
            .collect::<BTreeSet<_>>(),
        operation_fingerprints: operations
            .iter()
            .map(|operation| (operation.id.as_str().to_string(), operation.fingerprint))
            .collect::<BTreeMap<_, _>>(),
        operation_output_signatures: operations
            .iter()
            .filter_map(|operation| {
                operation_output_signature(spec, &operation.kind)
                    .map(|signature| (operation.id.as_str().to_string(), signature))
            })
            .collect(),
        operation_input_signatures: operations
            .iter()
            .map(|operation| {
                (
                    operation.id.as_str().to_string(),
                    operation_input_signature(spec, baseline, operation),
                )
            })
            .collect(),
    }
}

#[test]
fn plan_memo_computes_once_inside_a_plan_and_never_after_it() {
    let calls = Cell::new(0);
    let compute = || {
        calls.set(calls.get() + 1);
        "value".to_string()
    };
    // Outside a plan nothing is memoized.
    assert_eq!(plan_memo("signature-test".into(), compute), "value");
    assert_eq!(plan_memo("signature-test".into(), compute), "value");
    assert_eq!(calls.get(), 2);
    {
        let _plan = PlanMemoScope::begin();
        plan_memo("signature-test".into(), compute);
        plan_memo("signature-test".into(), compute);
        // A plan begun inside another shares its memo.
        let _nested = PlanMemoScope::begin();
        plan_memo("signature-test".into(), compute);
    }
    assert_eq!(calls.get(), 3, "one computation within the plan");
    // The plan ended: the next lookup reads afresh.
    plan_memo("signature-test".into(), compute);
    assert_eq!(calls.get(), 4);
}

#[test]
fn probe_cache_entries_expire_after_the_ttl() {
    let recent = (Instant::now(), "tool 1.0".to_string());
    assert_eq!(fresh(Some(&recent)), Some("tool 1.0".to_string()));
    let old = (
        Instant::now()
            .checked_sub(PROBE_CACHE_TTL + Duration::from_secs(1))
            .expect("instant in range"),
        "tool 0.9".to_string(),
    );
    assert_eq!(fresh(Some(&old)), None);
    assert_eq!(fresh(None), None);
}

#[test]
fn cached_signatures_equal_the_public_functions() {
    let scratch = TempDir::new("gaia-plan-signature-values");
    let (spec, baseline) = finished_build(&scratch);
    let signatures = PlanSignatures::new(&spec, &baseline);
    for (position, operation) in baseline.operations.iter().enumerate() {
        assert_eq!(
            signatures.input_signature(position),
            operation_input_signature(&spec, &baseline, operation),
            "input signature of {}",
            operation.id.as_str()
        );
        assert_eq!(
            signatures.output_signature(position),
            operation_output_signature(&spec, &operation.kind),
            "output signature of {}",
            operation.id.as_str()
        );
    }
}

#[test]
fn stage_items_hash_each_install_once_per_plan() {
    let scratch = TempDir::new("gaia-plan-signature-hashes");
    let (spec, baseline) = finished_build(&scratch);
    let state = recorded_state(&spec, &baseline);
    let (sources, artifacts, images) = gaia_default_providers::provider_catalogs();

    let before = CONTENT_HASHES.with(Cell::get);
    let plan = plan_build_with_reuse_state(&spec, &sources, &artifacts, &images, Some(&state));
    let hashes = CONTENT_HASHES.with(Cell::get) - before;

    // Every stage item depends on every install. Without the per-plan cache
    // each stage item hashed every install's artifact again.
    let reused_stage = plan
        .operations
        .iter()
        .filter(|operation| operation.id.as_str().starts_with("stage:file:"))
        .filter(|operation| matches!(operation.reuse, OperationReuse::Reuse { .. }))
        .count();
    let reasons = plan
        .operations
        .iter()
        .filter(|operation| operation.id.as_str().starts_with("stage:"))
        .map(|operation| format!("{} {:?}", operation.id.as_str(), operation.reuse))
        .collect::<Vec<_>>();
    // The default build's own stage file (`motd`) is one of them.
    let stage_files = spec.stage.files.len();
    assert_eq!(
        reused_stage, stage_files,
        "every stage item is reused: {reasons:#?}"
    );
    assert!(
        hashes <= INSTALLS + stage_files + 4,
        "{hashes} content hashes for {INSTALLS} installs and {stage_files} stage files"
    );
}
