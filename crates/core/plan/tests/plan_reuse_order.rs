pub mod support;

use gaia_config::resolve_config;
use gaia_plan::{OperationReuse, ReuseState, plan_build, plan_build_with_reuse_state};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use support::{provider_catalogs, reuse_state_for_ids, unique_dir};

/// An artifact that waits for the image prepare step. Planning pushes the
/// artifact before `image:prepare`, so the artifact is evaluated first.
fn split_build_spec() -> gaia_spec::ResolvedBuildSpec {
    let root_dir = unique_dir("gaia-plan-reuse-order-root");
    fs::create_dir_all(&root_dir).expect("root dir");
    let config_path = PathBuf::from(&root_dir).join("build.toml");
    fs::write(
        &config_path,
        r#"
build_name = "reuse-order"

[workspace]
root_dir = "."
build_dir = "build"
out_dir = "out"

[[sources]]
id = "buildroot-source"
kind = "path"
path = "."

[[artifacts]]
id = "gaia-app"
kind = "rust"
package = "gaia"
after_image_prepare = true
output_path = "out/gaia"

[image]
kind = "buildroot"
source = "buildroot-source"
defconfig = "qemu_aarch64_virt_defconfig"
"#,
    )
    .expect("config");
    resolve_config(config_path.to_str().expect("utf-8 config path"))
}

/// Writes the outputs the reusable operations check for, so the state built
/// from a baseline plan lets them reuse.
fn materialize_outputs(spec: &gaia_spec::ResolvedBuildSpec) {
    let source_dir = PathBuf::from(&spec.workspace.build_dir).join("sources/buildroot-source");
    fs::create_dir_all(&source_dir).expect("source dir");
    fs::write(source_dir.join("source.txt"), "ok").expect("source marker");
    let artifact_output = &spec.artifacts[0].output.path;
    fs::create_dir_all(PathBuf::from(artifact_output).parent().expect("parent"))
        .expect("artifact output dir");
    fs::write(artifact_output, "artifact").expect("artifact output");
    fs::create_dir_all(
        PathBuf::from(&spec.workspace.build_dir).join("image/buildroot-output/target"),
    )
    .expect("buildroot target dir");
}

fn reuse_of<'plan>(plan: &'plan gaia_plan::ExecutionPlan, id: &str) -> &'plan OperationReuse {
    &plan
        .operations
        .iter()
        .find(|operation| operation.id.as_str() == id)
        .unwrap_or_else(|| panic!("operation {id} in plan"))
        .reuse
}

#[test]
fn dependency_reused_after_dependent_is_evaluated_does_not_cascade() {
    let spec = split_build_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    materialize_outputs(&spec);
    let baseline = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let state: ReuseState = reuse_state_for_ids(
        &spec,
        &baseline,
        &[
            "source:buildroot-source",
            "image:prepare",
            "artifact:gaia-app",
        ],
    );
    let position = |id: &str| {
        baseline
            .operations
            .iter()
            .position(|operation| operation.id.as_str() == id)
            .expect("operation in baseline")
    };
    assert!(
        position("artifact:gaia-app") < position("image:prepare"),
        "the regression needs the artifact planned before its dependency"
    );

    let plan = plan_build_with_reuse_state(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(&state),
    );

    assert!(matches!(
        reuse_of(&plan, "image:prepare"),
        OperationReuse::Reuse { .. }
    ));
    assert!(matches!(
        reuse_of(&plan, "artifact:gaia-app"),
        OperationReuse::Reuse { .. }
    ));
    let summary = gaia_plan::invalidation_summary(&plan);
    assert!(
        !summary.cascaded.contains(&"artifact:gaia-app".to_string()),
        "a reused dependency must not make its dependent run: {summary:?}"
    );
}

#[test]
fn dependency_that_runs_still_cascades_to_a_planned_after_dependent() {
    let spec = split_build_spec();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    materialize_outputs(&spec);
    let baseline = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);
    let mut state = reuse_state_for_ids(
        &spec,
        &baseline,
        &["source:buildroot-source", "artifact:gaia-app"],
    );
    // Not in the state, so image:prepare executes and its dependent follows.
    state.completed_operation_ids = BTreeSet::from([
        "source:buildroot-source".to_string(),
        "artifact:gaia-app".to_string(),
    ]);

    let plan = plan_build_with_reuse_state(
        &spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(&state),
    );

    let OperationReuse::Execute(prepare) = reuse_of(&plan, "image:prepare") else {
        panic!("image:prepare must execute");
    };
    assert_eq!(prepare.code, "not_in_reuse_state");
    let OperationReuse::Execute(artifact) = reuse_of(&plan, "artifact:gaia-app") else {
        panic!("artifact must execute after a dependency that runs");
    };
    assert_eq!(artifact.code, "dependency_rebuilt");
    assert_eq!(
        artifact.message,
        "operation 'artifact:gaia-app' will execute because image:prepare runs (not_in_reuse_state)"
    );
}
