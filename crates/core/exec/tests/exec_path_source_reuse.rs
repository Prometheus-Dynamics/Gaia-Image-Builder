pub mod support;

use gaia_plan::{OperationReuse, plan_build, plan_build_with_reuse_state};
use std::fs;
use std::path::PathBuf;
use support::{fixture_spec, provider_catalogs, reuse_state_for_ids};

const ROOT_SOURCE: &str = "source:workspace-root";

/// Build and out live inside the source root, as they do in a real workspace.
fn spec_with_state_inside_root() -> gaia_spec::ResolvedBuildSpec {
    let mut spec = fixture_spec();
    let root = PathBuf::from(&spec.workspace.root_dir);
    spec.workspace.build_dir = root.join("build").display().to_string();
    spec.workspace.out_dir = root.join("out").display().to_string();
    spec
}

/// What a successful first run leaves behind inside the root.
fn simulate_run_state(spec: &gaia_spec::ResolvedBuildSpec) {
    let root = PathBuf::from(&spec.workspace.root_dir);
    let materialized = PathBuf::from(&spec.workspace.build_dir).join("sources/workspace-root");
    fs::create_dir_all(&materialized).expect("materialized source dir");
    fs::write(materialized.join("source.txt"), "ok").expect("source marker");
    fs::create_dir_all(root.join(".gaia/runtime")).expect("gaia state dir");
    fs::write(root.join(".gaia/runtime/state.txt"), "run").expect("gaia state");
    fs::create_dir_all(root.join(".gaia-trash/old")).expect("trash dir");
    fs::write(root.join(".gaia-run.status.json"), "{}").expect("run status file");
    fs::create_dir_all(PathBuf::from(&spec.workspace.out_dir).join("images")).expect("out dir");
    fs::write(
        PathBuf::from(&spec.workspace.out_dir).join("images/image.txt"),
        "image",
    )
    .expect("out file");
}

fn root_source_reuse(
    spec: &gaia_spec::ResolvedBuildSpec,
    baseline: &gaia_plan::ExecutionPlan,
) -> OperationReuse {
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let reuse_state = reuse_state_for_ids(spec, baseline, &[ROOT_SOURCE]);
    let plan = plan_build_with_reuse_state(
        spec,
        &source_catalog,
        &artifact_catalog,
        &image_catalog,
        Some(&reuse_state),
    );
    plan.operations
        .iter()
        .find(|operation| operation.id.as_str() == ROOT_SOURCE)
        .expect("workspace-root source operation")
        .reuse
        .clone()
}

#[test]
fn workspace_root_source_is_reused_after_run_creates_gaia_state_in_root() {
    let spec = spec_with_state_inside_root();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    simulate_run_state(&spec);

    assert!(
        matches!(
            root_source_reuse(&spec, &baseline),
            OperationReuse::Reuse { .. }
        ),
        "workspace-root should be reused after the run's own state appears in the root"
    );
}

#[test]
fn workspace_root_source_rebuilds_when_a_real_source_file_changes() {
    let spec = spec_with_state_inside_root();
    let (source_catalog, artifact_catalog, image_catalog) = provider_catalogs();
    let baseline = plan_build(&spec, &source_catalog, &artifact_catalog, &image_catalog);

    simulate_run_state(&spec);
    fs::write(
        PathBuf::from(&spec.workspace.root_dir).join("notes.txt"),
        "edited by the user",
    )
    .expect("source edit");

    assert!(
        matches!(
            root_source_reuse(&spec, &baseline),
            OperationReuse::Execute(_)
        ),
        "a file added under the source root must change its fingerprint"
    );
}
