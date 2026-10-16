//! Content of what the image feed delivers: a staged unit referenced through
//! `@assets/...` and a rebuilt artifact's bytes must reach the fingerprints
//! and the input signatures image:build's early cutoff compares.
pub mod support;

use gaia_plan::{
    ExecutionPlan, OperationKind, OperationReuse, ReuseState, operation_content_signature,
    operation_input_signature, plan_build, plan_build_with_reuse_state,
};
use gaia_spec::{ResolvedBuildSpec, StageServiceSpec};
use gaia_testkit::TempDir;
use std::fs;
use std::path::{Path, PathBuf};
use support::{provider_catalogs, test_spec};

const UNIT_ID: &str = "gaia-service";
const ARTIFACT_ID: &str = "gaia-app";

/// The default config with `@assets` moved into the scratch directory and one
/// stage service whose unit file is `@assets/systemd/gaia.service`.
fn feed_spec(scratch: &TempDir) -> (ResolvedBuildSpec, PathBuf) {
    let mut spec = test_spec(scratch);
    let assets = scratch.join("assets");
    fs::create_dir_all(assets.join("systemd")).expect("assets dir");
    for named in &mut spec.workspace.named_paths {
        if named.alias == "assets" {
            named.path = assets.display().to_string();
        }
    }
    let unit = assets.join("systemd/gaia.service");
    write(&unit, "[Service]\nExecStart=/usr/bin/gaia-v1\n");
    spec.stage.services = vec![StageServiceSpec::new(
        UNIT_ID,
        "gaia.service",
        "@assets/systemd/gaia.service",
    )];
    (spec, unit)
}

fn write(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write fixture");
}

/// Outputs and runtime state a completed build leaves, so that an unchanged
/// plan reuses everything in `ids`.
fn materialize_outputs(spec: &ResolvedBuildSpec) {
    let build = PathBuf::from(&spec.workspace.build_dir);
    for source in ["gaia-upstream", "workspace-root"] {
        fs::create_dir_all(build.join("sources").join(source)).expect("source dir");
        write(&build.join("sources").join(source).join("source.txt"), "ok");
    }
    let artifact = spec.artifacts[0].output.path.clone();
    fs::create_dir_all(Path::new(&artifact).parent().expect("parent")).expect("artifact dir");
    write(Path::new(&artifact), "artifact v1");
    let collect = spec.image.output.collect_dir.clone().expect("collect dir");
    fs::create_dir_all(&collect).expect("collect");
    write(&Path::new(&collect).join("image-provider.txt"), "image");
    let archive = spec.image.output.archive_name.clone().expect("archive");
    write(&Path::new(&collect).join(archive), "archive");
    fs::create_dir_all(build.join("image/buildroot-output/target")).expect("target");
    let runtime = PathBuf::from(&spec.workspace.out_dir).join(".gaia/runtime");
    fs::create_dir_all(&runtime).expect("runtime");
    write(
        &runtime.join(format!("install-install-{ARTIFACT_ID}.state")),
        &format!(
            "kind=install\ninstall_id=install-{ARTIFACT_ID}\nartifact_id={ARTIFACT_ID}\ndest=/usr/bin/app\n"
        ),
    );
    write(
        &runtime.join(format!("stage-service-{UNIT_ID}.state")),
        &format!("kind=stage-service\nitem_id={UNIT_ID}\nname=gaia.service\n"),
    );
    write(
        &runtime.join("stage-file-motd.state"),
        "kind=stage-file\nitem_id=motd\ndest=/etc/motd\n",
    );
    write(
        &runtime.join("stage-env-runtime-env.state"),
        "kind=stage-env\nitem_id=runtime-env\nname=runtime\nentry_count=2\n",
    );
    write(
        &runtime.join("checkpoint-base-image.state"),
        "kind=checkpoint\ncheckpoint_id=base-image\nbackend=local\n",
    );
}

/// Every operation of `plan` completed, with the signatures a finished run
/// records (fingerprint, output and input signatures).
fn recorded_state(spec: &ResolvedBuildSpec, plan: &ExecutionPlan) -> ReuseState {
    let ids = plan
        .operations
        .iter()
        .map(|operation| operation.id.as_str().to_string())
        .filter(|id| id != "resolve-build" && id != "report:emit")
        .collect::<Vec<_>>();
    let refs = ids.iter().map(String::as_str).collect::<Vec<_>>();
    let mut state = support::reuse_state_for_ids(spec, plan, &refs);
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

fn reuse_of<'plan>(plan: &'plan ExecutionPlan, id: &str) -> &'plan OperationReuse {
    &plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == id)
        .unwrap_or_else(|| panic!("operation {id} in plan"))
        .reuse
}

fn operation<'plan>(plan: &'plan ExecutionPlan, id: &str) -> &'plan gaia_plan::PlannedOperation {
    plan.operations
        .iter()
        .find(|operation| operation.id.as_str() == id)
        .unwrap_or_else(|| panic!("operation {id} in plan"))
}

#[test]
fn unit_content_resolves_through_the_assets_alias() {
    let scratch = TempDir::new("gaia-plan-stage-content");
    let (spec, _unit) = feed_spec(&scratch);
    let kind = OperationKind::RenderStageService {
        item_id: UNIT_ID.into(),
    };
    let signature = operation_content_signature(&spec, &kind).expect("content signature");
    assert!(
        signature.contains("sha256:"),
        "the @assets unit must hash its content: {signature}"
    );
    assert!(!signature.contains("unresolved"), "{signature}");
}

#[test]
fn unit_content_change_reruns_the_stage_service_and_image_build() {
    let scratch = TempDir::new("gaia-plan-stage-content");
    let (spec, unit) = feed_spec(&scratch);
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    materialize_outputs(&spec);
    let baseline = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let state = recorded_state(&spec, &baseline);

    // Same bytes, new mtime: reused.
    write(&unit, "[Service]\nExecStart=/usr/bin/gaia-v1\n");
    let unchanged = plan_build_with_reuse_state(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(&state),
    );
    assert!(
        matches!(
            reuse_of(&unchanged, "stage:service:gaia-service"),
            OperationReuse::Reuse { .. }
        ),
        "{:?}",
        reuse_of(&unchanged, "stage:service:gaia-service")
    );

    // New bytes: the stage service reruns, and so does the image built from it.
    write(
        &unit,
        "[Service]\nExecStart=/usr/bin/gaia-v2-with-more-text\n",
    );
    let changed = plan_build_with_reuse_state(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(&state),
    );
    assert!(matches!(
        reuse_of(&changed, "stage:service:gaia-service"),
        OperationReuse::Execute(reason) if reason.code == "operation_fingerprint_mismatch"
    ));
    assert!(
        matches!(
            reuse_of(&changed, "image:build"),
            OperationReuse::Execute(_)
        ),
        "image:build must rerun for a changed staged unit: {:?}",
        reuse_of(&changed, "image:build")
    );
}

#[test]
fn unit_content_change_changes_the_image_build_input_signature() {
    let scratch = TempDir::new("gaia-plan-stage-content");
    let (spec, unit) = feed_spec(&scratch);
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    materialize_outputs(&spec);
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let image = operation(&plan, "image:build");
    assert!(
        image
            .depends_on
            .iter()
            .any(|dependency| dependency.as_str() == "stage:service:gaia-service"),
        "image:build reads the staged unit through its stage operation"
    );
    let before = operation_input_signature(&spec, &plan, image);

    write(
        &unit,
        "[Service]\nExecStart=/usr/bin/gaia-v2-with-more-text\n",
    );
    let after = operation_input_signature(&spec, &plan, image);
    assert_ne!(before, after, "the cutoff must see the new unit bytes");

    write(
        &unit,
        "[Service]\nExecStart=/usr/bin/gaia-v2-with-more-text\n",
    );
    assert_eq!(after, operation_input_signature(&spec, &plan, image));
}

#[test]
fn rebuilt_artifact_bytes_reach_the_install_and_image_build_inputs() {
    let scratch = TempDir::new("gaia-plan-stage-content");
    let (spec, _unit) = feed_spec(&scratch);
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    materialize_outputs(&spec);
    let plan = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let install = operation(&plan, &format!("install:install-{ARTIFACT_ID}"));
    let image = operation(&plan, "image:build");
    let install_before = operation_content_signature(&spec, &install.kind);
    let image_before = operation_input_signature(&spec, &plan, image);
    let artifact = PathBuf::from(&spec.artifacts[0].output.path);

    // A rebuild with different bytes.
    write(&artifact, "artifact v2, rebuilt with different bytes");
    assert_ne!(
        install_before,
        operation_content_signature(&spec, &install.kind),
        "the install delivers the artifact's bytes"
    );
    assert_ne!(
        image_before,
        operation_input_signature(&spec, &plan, image),
        "a rebuilt artifact must not be cut off at image:build"
    );

    // A rebuild with identical bytes may be cut off.
    write(&artifact, "artifact v1");
    assert_eq!(
        install_before,
        operation_content_signature(&spec, &install.kind)
    );
    assert_eq!(image_before, operation_input_signature(&spec, &plan, image));
}
