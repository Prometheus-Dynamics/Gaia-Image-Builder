//! Synthetic planning benchmark (ignored by default). Many installs and stage
//! files, a Buildroot tree of about 20k files, and a recorded reuse state, so
//! the timed plan reuses almost everything and runs the reuse decisions.
//!
//! Run: `cargo test --release -p gaia-plan --test plan_bench_synthetic -- --ignored --nocapture`
pub mod support;

use gaia_plan::{
    ExecutionPlan, OperationReuse, ReuseState, operation_input_signature,
    plan_build_with_reuse_state,
};
use gaia_spec::{InstallEntrySpec, ResolvedBuildSpec, StageContentOriginSpec, StageFileSpec};
use gaia_testkit::TempDir;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Instant;
use support::{provider_catalogs, reuse_state_for_ids, test_spec};

const INSTALLS: usize = 40;
const STAGE_FILES: usize = 60;
const BUILDROOT_DIRS: usize = 100;
const BUILDROOT_FILES_PER_DIR: usize = 200;
const ARTIFACT_BYTES: usize = 8 << 20;

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    fs::write(path, contents).expect("write fixture");
}

fn recorded_state(spec: &ResolvedBuildSpec, plan: &ExecutionPlan) -> ReuseState {
    let ids = plan
        .operations
        .iter()
        .map(|operation| operation.id.as_str().to_string())
        .filter(|id| id != "resolve-build" && id != "report:emit")
        .collect::<Vec<_>>();
    let refs = ids.iter().map(String::as_str).collect::<Vec<_>>();
    let mut state = reuse_state_for_ids(spec, plan, &refs);
    state.operation_input_signatures = plan
        .operations
        .iter()
        .filter(|operation| ids.iter().any(|id| id == operation.id.as_str()))
        .map(|operation| {
            (
                operation.id.as_str().to_string(),
                operation_input_signature(spec, plan, operation),
            )
        })
        .collect();
    state
}

#[test]
#[ignore = "timing benchmark; run in release with --ignored --nocapture"]
fn synthetic_plan_timing() {
    let scratch = TempDir::new("gaia-plan-bench");
    let mut spec = test_spec(&scratch);

    // A large artifact, so each install's content hash has a real cost.
    let artifact = spec.artifacts[0].clone();
    let artifact_path = PathBuf::from(&artifact.output.path);
    write(&artifact_path, &"x".repeat(ARTIFACT_BYTES));

    // Many installs of that artifact, and stage files that depend on all of them.
    let assets = scratch.join("assets");
    for index in 0..INSTALLS {
        spec.install.entries.push(InstallEntrySpec::new(
            format!("install-bench-{index}"),
            artifact.id.clone(),
            format!("/usr/bin/bench-{index}"),
        ));
    }
    for index in 0..STAGE_FILES {
        let src = assets.join(format!("bench-{index}.txt"));
        write(&src, &format!("payload {index}\n"));
        spec.stage.files.push(StageFileSpec::new(
            format!("bench-file-{index}"),
            src.display().to_string(),
            format!("/etc/bench-{index}"),
            StageContentOriginSpec::StaticAsset,
        ));
    }

    // Outputs and runtime state of a finished run, so reuse is what gets timed.
    let build = PathBuf::from(&spec.workspace.build_dir);
    for source in &spec.sources {
        let dir = gaia_spec::source_build_dir_by_id(&spec, source.id.as_str());
        write(&dir.join("source.txt"), "ok");
    }
    let collect = spec.image.output.collect_dir.clone().expect("collect dir");
    write(&Path::new(&collect).join("image-provider.txt"), "image");
    let archive = spec.image.output.archive_name.clone().expect("archive");
    write(&Path::new(&collect).join(archive), "archive");
    fs::create_dir_all(build.join("image/buildroot-output/target")).expect("target");
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

    // A Buildroot tree of about 20k files, hashed by the image backend signature.
    let buildroot = scratch.join("buildroot");
    for dir in 0..BUILDROOT_DIRS {
        for file in 0..BUILDROOT_FILES_PER_DIR {
            write(
                &buildroot.join(format!("d{dir}/f{file}.txt")),
                "buildroot tree file\n",
            );
        }
    }
    // SAFETY: this is the only test in this binary; no other thread reads the
    // environment concurrently.
    unsafe { std::env::set_var("GAIA_BUILDROOT_DIR", &buildroot) };

    let (sources, artifacts, images) = provider_catalogs();
    let baseline = plan_build_with_reuse_state(&spec, &sources, &artifacts, &images, None);
    let state = recorded_state(&spec, &baseline);

    let mut best = u128::MAX;
    let mut last: Option<ExecutionPlan> = None;
    for round in 0..3 {
        let started = Instant::now();
        let plan = plan_build_with_reuse_state(&spec, &sources, &artifacts, &images, Some(&state));
        let elapsed = started.elapsed().as_millis();
        println!("plan round {round}: {elapsed} ms");
        best = best.min(elapsed);
        last = Some(plan);
    }
    let plan = last.expect("plan");
    // Digest of every decision (id and reuse reason), so two builds can be
    // compared. Fingerprints are left out: they hash the random scratch path.
    let mut digest = std::collections::hash_map::DefaultHasher::new();
    for operation in &plan.operations {
        operation.id.as_str().hash(&mut digest);
        format!("{:?}", operation.reuse).hash(&mut digest);
    }
    println!("decision digest: {:016x}", digest.finish());
    let reused = plan
        .operations
        .iter()
        .filter(|operation| matches!(operation.reuse, OperationReuse::Reuse { .. }))
        .count();
    println!(
        "operations: {} total, {} reused; best of 3: {best} ms",
        plan.operations.len(),
        reused
    );
    assert!(reused > 0, "the benchmark must exercise reuse");
}
